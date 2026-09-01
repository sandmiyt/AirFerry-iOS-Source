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
    private var processing = false

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
        guard session.inputs.isEmpty || session.outputs.isEmpty else { return nil }

        session.beginConfiguration()
        defer { session.commitConfiguration() }

        session.inputs.forEach(session.removeInput)
        session.outputs.forEach(session.removeOutput)
        if session.canSetSessionPreset(.hd1920x1080) {
            session.sessionPreset = .hd1920x1080
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
        if let connection = output.connection(with: .video),
           connection.isVideoRotationAngleSupported(90) {
            connection.videoRotationAngle = 90
        }

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
        return nil
    }

    func captureOutput(
        _ output: AVCaptureOutput,
        didOutput sampleBuffer: CMSampleBuffer,
        from connection: AVCaptureConnection
    ) {
        guard !processing else { return }
        processing = true
        let request = VNDetectBarcodesRequest { [weak self] request, _ in
            defer { self?.processing = false }
            guard let observations = request.results as? [VNBarcodeObservation] else { return }
            for observation in observations where observation.symbology == .qr {
                if let payload = observation.payloadData, !payload.isEmpty {
                    self?.onPayload?(payload)
                }
            }
        }
        request.symbologies = [.qr]
        do {
            try VNImageRequestHandler(cmSampleBuffer: sampleBuffer, orientation: .up).perform([request])
        } catch {
            processing = false
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
    }
}
