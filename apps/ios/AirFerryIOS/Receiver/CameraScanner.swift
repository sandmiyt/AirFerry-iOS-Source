import AVFoundation
import ImageIO
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
    private var isConfigured = false
    private var lastScanUptime = 0.0
    private var consecutiveVisionFailures = 0
    private lazy var barcodeRequest: VNDetectBarcodesRequest = {
        let request = VNDetectBarcodesRequest()
        request.symbologies = [.qr]
        if #available(iOS 26.0, *) {
            // iOS 26 的部分 iPhone 16 设备存在 Vision/ANE 条码模型失效问题。
            // 二维码解码量不大，接收页优先使用 CPU，避免系统模型异常拖垮扫码链路。
            request.usesCPUOnly = true
        }
        return request
    }()

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

    private func configureAndRun() {
        captureQueue.async { [weak self] in
            guard let self else { return }
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
        if session.canSetSessionPreset(.hd1280x720) {
            session.sessionPreset = .hd1280x720
        } else {
            session.sessionPreset = .high
        }

        guard
            let camera = AVCaptureDevice.default(.builtInWideAngleCamera, for: .video, position: .back),
            let input = try? AVCaptureDeviceInput(device: camera),
            session.canAddInput(input)
        else {
            return "无法打开后置摄像头。"
        }

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
        guard now - lastScanUptime >= 1.0 / 12.0,
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
                if consecutiveVisionFailures == 24 {
                    DispatchQueue.main.async { [weak self] in
                        self?.state = .failed("系统扫码引擎连续失败，请重启 iPhone 后再试。")
                    }
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
