import AVFoundation
import ImageIO
import OSLog
import SwiftUI
import UIKit
import Vision

final class CameraScanner: NSObject, ObservableObject, AVCaptureVideoDataOutputSampleBufferDelegate {
    enum State: Equatable {
        case idle
        case denied
        case running
        case failed(String)
    }

    let session = AVCaptureSession()
    @Published private(set) var state: State = .idle
    var onPayload: ((Data) -> Void)?

    private let captureQueue = DispatchQueue(label: "local.airferry.camera.capture", qos: .userInitiated)
    private let visionQueue = DispatchQueue(label: "local.airferry.camera.vision", qos: .userInitiated)
    private let logger = Logger(subsystem: "local.airferry.ios", category: "CameraScanner")
    private var runtimeErrorObserver: NSObjectProtocol?
    private var isConfigured = false
    private var lastScanUptime = 0.0
    private var consecutiveVisionFailures = 0
    private lazy var barcodeRequest = makeBarcodeRequest()

    override init() {
        super.init()
        runtimeErrorObserver = NotificationCenter.default.addObserver(
            forName: AVCaptureSession.runtimeErrorNotification,
            object: session,
            queue: nil
        ) { [weak self] notification in
            self?.handleRuntimeError(notification)
        }
    }

    deinit {
        if let runtimeErrorObserver {
            NotificationCenter.default.removeObserver(runtimeErrorObserver)
        }
    }

    private func makeBarcodeRequest() -> VNDetectBarcodesRequest {
        let request = VNDetectBarcodesRequest()
        request.symbologies = [.qr]
        request.revision = VNDetectBarcodesRequestRevision3
        return request
    }

    private func handleRuntimeError(_ notification: Notification) {
        let error = notification.userInfo?[AVCaptureSessionErrorKey] as? AVError
        logger.error("Capture session runtime error: \(error?.localizedDescription ?? "unknown", privacy: .public)")
        if error?.code == .mediaServicesWereReset {
            configureAndRun(forceReconfigure: true)
        } else {
            DispatchQueue.main.async { [weak self] in
                self?.state = .failed("系统相机服务暂时不可用，请点击重新打开相机。")
            }
        }
    }

    func start() {
        switch AVCaptureDevice.authorizationStatus(for: .video) {
        case .authorized:
            configureAndRun()
        case .notDetermined:
            AVCaptureDevice.requestAccess(for: .video) { [weak self] allowed in
                DispatchQueue.main.async {
                    allowed ? self?.configureAndRun() : (self?.state = .denied)
                }
            }
        default:
            state = .denied
        }
    }

    func stop() {
        captureQueue.async { [session] in
            if session.isRunning { session.stopRunning() }
        }
    }

    func retry() {
        configureAndRun(forceReconfigure: true)
    }

    private func configureAndRun(forceReconfigure: Bool = false) {
        captureQueue.async { [weak self] in
            guard let self else { return }
            if forceReconfigure {
                if session.isRunning { session.stopRunning() }
                isConfigured = false
            }
            if let message = configureSessionIfNeeded() {
                DispatchQueue.main.async { self.state = .failed(message) }
                return
            }
            if !session.isRunning { session.startRunning() }
            let isRunning = session.isRunning
            DispatchQueue.main.async {
                self.state = isRunning
                    ? .running
                    : .failed("相机启动失败，请返回后重试。")
            }
        }
    }

    private func configureSessionIfNeeded() -> String? {
        guard !isConfigured else { return nil }

        session.beginConfiguration()
        defer { session.commitConfiguration() }

        session.inputs.forEach(session.removeInput)
        session.outputs.forEach(session.removeOutput)
        // Dense binary QR symbols need more source pixels than ordinary URL
        // codes. Prefer 1080p and fall back only on devices that cannot supply
        // it; 720p made V22/V27 streams look valid in preview but undecodable.
        if session.canSetSessionPreset(.hd1920x1080) {
            session.sessionPreset = .hd1920x1080
        } else if session.canSetSessionPreset(.hd1280x720) {
            session.sessionPreset = .hd1280x720
        } else {
            session.sessionPreset = .high
        }

        guard let camera = AVCaptureDevice.default(
            .builtInWideAngleCamera,
            for: .video,
            position: .back
        ) else {
            return "无法打开后置摄像头。"
        }

        let input: AVCaptureDeviceInput
        do {
            input = try AVCaptureDeviceInput(device: camera)
        } catch {
            return "无法连接摄像头：\(error.localizedDescription)"
        }
        guard session.canAddInput(input) else { return "系统拒绝添加摄像头输入。" }

        session.addInput(input)
        let output = AVCaptureVideoDataOutput()
        output.alwaysDiscardsLateVideoFrames = true
        output.videoSettings = [
            kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
        ]
        output.setSampleBufferDelegate(self, queue: visionQueue)
        guard session.canAddOutput(output) else {
            session.removeInput(input)
            return "无法创建扫码输出。"
        }

        session.addOutput(output)

        do {
            try camera.lockForConfiguration()
            defer { camera.unlockForConfiguration() }
            if camera.isFocusModeSupported(.continuousAutoFocus) {
                camera.focusMode = .continuousAutoFocus
            }
            if camera.isExposureModeSupported(.continuousAutoExposure) {
                camera.exposureMode = .continuousAutoExposure
            }
        } catch {
            // 对焦锁失败不影响相机采集，保留系统默认配置继续启动。
        }
        isConfigured = true
        return nil
    }

    func captureOutput(
        _ output: AVCaptureOutput,
        didOutput sampleBuffer: CMSampleBuffer,
        from connection: AVCaptureConnection
    ) {
        let now = ProcessInfo.processInfo.systemUptime
        // The delegate queue is serial and late video frames are discarded, so
        // Vision already provides natural back-pressure. A 30 Hz ceiling gives
        // the scanner more clean-frame opportunities without queue buildup.
        guard now - lastScanUptime >= 1.0 / 30.0,
              let pixelBuffer = CMSampleBufferGetImageBuffer(sampleBuffer)
        else { return }
        lastScanUptime = now

        autoreleasepool {
            do {
                let handler = VNImageRequestHandler(
                    cvPixelBuffer: pixelBuffer,
                    orientation: .right,
                    options: [:]
                )
                try handler.perform([barcodeRequest])
                consecutiveVisionFailures = 0
            } catch {
                consecutiveVisionFailures += 1
                logger.error("Vision QR detection failed: \(error.localizedDescription, privacy: .public)")
                if consecutiveVisionFailures >= 3 {
                    barcodeRequest = makeBarcodeRequest()
                    consecutiveVisionFailures = 0
                }
                return
            }

            guard let observations = barcodeRequest.results as? [VNBarcodeObservation] else { return }
            for observation in observations where observation.symbology == .qr {
                if let payload = observation.payloadData, !payload.isEmpty {
                    onPayload?(payload)
                }
            }
        }
    }
}

struct CameraPreview: UIViewRepresentable {
    let session: AVCaptureSession

    func makeUIView(context: Context) -> PreviewView {
        let view = PreviewView()
        view.previewLayer.session = session
        view.previewLayer.videoGravity = .resizeAspectFill
        return view
    }

    func updateUIView(_ uiView: PreviewView, context: Context) {}

    final class PreviewView: UIView {
        override class var layerClass: AnyClass { AVCaptureVideoPreviewLayer.self }
        var previewLayer: AVCaptureVideoPreviewLayer { layer as! AVCaptureVideoPreviewLayer }

        override func layoutSubviews() {
            super.layoutSubviews()
            if let connection = previewLayer.connection,
               connection.isVideoRotationAngleSupported(90) {
                connection.videoRotationAngle = 90
            }
        }
    }
}
