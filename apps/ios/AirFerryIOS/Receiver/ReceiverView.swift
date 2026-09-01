import SwiftUI

struct ReceiverView: View {
    @ObservedObject var model: ReceiverViewModel
    @ObservedObject private var camera: CameraScanner

    init(model: ReceiverViewModel) {
        self.model = model
        camera = model.camera
    }

    var body: some View {
        ZStack {
            AirFerryBackdrop()

            ScrollView {
                VStack(spacing: 18) {
                    receiverHero
                    cameraSurface
                    statusCard
                    if let file = model.completedFile {
                        completedActions(file)
                    }
                }
                .frame(maxWidth: 720)
                .frame(maxWidth: .infinity)
                .padding(.horizontal, 16)
                .padding(.top, 8)
                .padding(.bottom, 34)
            }
        }
        .navigationTitle("接收")
        .navigationBarTitleDisplayMode(.inline)
        .onAppear { model.start() }
        .onDisappear { model.stop() }
        .sheet(item: $model.previewFile) { file in
            QuickLookPreview(url: file.url)
                .ignoresSafeArea()
        }
        .alert("接收失败", isPresented: Binding(
            get: { model.errorMessage != nil },
            set: { if !$0 { model.errorMessage = nil } }
        )) {
            Button("重试") { model.reset() }
            Button("取消", role: .cancel) { model.errorMessage = nil }
        } message: {
            Text(model.errorMessage ?? "未知错误")
        }
    }

    private var receiverHero: some View {
        HStack(spacing: 14) {
            Image(systemName: "viewfinder.circle.fill")
                .font(.system(size: 38))
                .symbolRenderingMode(.palette)
                .foregroundStyle(.white, AirFerryTheme.accent)
            VStack(alignment: .leading, spacing: 3) {
                Text("扫描接收")
                    .font(.title2.bold())
                Text("让动态二维码完整保持在取景框内")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            }
            Spacer()
        }
    }

    @ViewBuilder
    private var cameraSurface: some View {
        Group {
            switch camera.state {
            case .denied:
                ContentUnavailableView(
                    "需要相机权限",
                    systemImage: "camera.fill",
                    description: Text("请在“设置 → AirFerry”中允许相机访问。")
                )
            case .failed(let message):
                ContentUnavailableView(
                    "相机不可用",
                    systemImage: "camera.fill",
                    description: Text(message)
                )
            default:
                CameraPreview(session: camera.session)
                    .overlay { ScannerFrame() }
            }
        }
        .frame(maxWidth: 520)
        .aspectRatio(1, contentMode: .fit)
        .background(Color.black.opacity(0.88))
        .clipShape(RoundedRectangle(cornerRadius: 28, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 28, style: .continuous)
                .stroke(.white.opacity(0.30), lineWidth: 0.8)
        }
        .shadow(color: AirFerryTheme.accent.opacity(0.18), radius: 24, y: 12)
    }

    private var statusCard: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 10) {
                Image(systemName: model.completedFile == nil ? "wave.3.right.circle" : "checkmark.circle.fill")
                    .font(.title2)
                    .foregroundStyle(model.completedFile == nil ? AirFerryTheme.accent : .green)
                Text(model.filename)
                    .font(.headline)
                    .lineLimit(2)
                Spacer()
                Text(model.progress, format: .percent.precision(.fractionLength(0)))
                    .font(.subheadline.monospacedDigit().weight(.semibold))
            }

            ProgressView(value: model.progress)
                .tint(AirFerryTheme.accent)

            Text(model.status)
                .font(.subheadline)
                .foregroundStyle(.secondary)

            if model.segmentCount > 1 {
                Label(
                    "已收分段 \(model.receivedSegments) / \(model.segmentCount)",
                    systemImage: "square.stack.3d.up.fill"
                )
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
            }
        }
        .padding(18)
        .airFerryGlassCard()
    }

    private func completedActions(_ file: ReceivedFile) -> some View {
        AirFerryGlassGroup(spacing: 12) {
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 12) {
                    Button("直接查看") { model.previewFile = file }
                        .airFerryPrimaryButton()
                    ShareLink(item: file.url) {
                        Label("分享", systemImage: "square.and.arrow.up")
                    }
                    .airFerrySecondaryButton()
                    Button("接收下一个") { model.reset() }
                        .airFerrySecondaryButton()
                }

                VStack(spacing: 10) {
                    Button("直接查看") { model.previewFile = file }
                        .frame(maxWidth: .infinity)
                        .airFerryPrimaryButton()
                    ShareLink(item: file.url) {
                        Label("分享或导出", systemImage: "square.and.arrow.up")
                            .frame(maxWidth: .infinity)
                    }
                    .airFerrySecondaryButton()
                    Button("接收下一个") { model.reset() }
                        .frame(maxWidth: .infinity)
                        .airFerrySecondaryButton()
                }
            }
        }
    }
}

private struct ScannerFrame: View {
    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 28, style: .continuous)
                .stroke(.white.opacity(0.92), lineWidth: 3)
                .padding(34)

            LinearGradient(
                colors: [.clear, AirFerryTheme.cyan.opacity(0.95), .clear],
                startPoint: .leading,
                endPoint: .trailing
            )
            .frame(height: 2)
            .padding(.horizontal, 48)
            .shadow(color: AirFerryTheme.cyan, radius: 8)
        }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }
}
