import SwiftUI

struct ReceiverView: View {
    @ObservedObject var model: ReceiverViewModel
    @ObservedObject private var camera: CameraScanner

    init(model: ReceiverViewModel) {
        self.model = model
        camera = model.camera
    }

    var body: some View {
        VStack(spacing: 14) {
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
                        .overlay {
                            RoundedRectangle(cornerRadius: 22)
                                .stroke(.white.opacity(0.85), lineWidth: 3)
                                .padding(28)
                        }
                        .clipShape(RoundedRectangle(cornerRadius: 20))
                }
            }
            .frame(maxHeight: .infinity)

            VStack(alignment: .leading, spacing: 8) {
                Text(model.filename).font(.headline).lineLimit(1)
                ProgressView(value: model.progress)
                HStack {
                    Text(model.status)
                    Spacer()
                    Text(model.progress, format: .percent.precision(.fractionLength(0)))
                        .monospacedDigit()
                }
                .font(.caption)
                .foregroundStyle(.secondary)
                if model.segmentCount > 1 {
                    Text("已收分段 \(model.receivedSegments) / \(model.segmentCount)")
                        .font(.caption.monospacedDigit())
                }
            }

            if let file = model.completedFile {
                HStack {
                    Button("直接查看") { model.previewFile = file }
                        .buttonStyle(.borderedProminent)
                    ShareLink(item: file.url) {
                        Label("分享/导出", systemImage: "square.and.arrow.up")
                    }
                    .buttonStyle(.bordered)
                    Button("接收下一个") { model.reset() }
                        .buttonStyle(.bordered)
                }
            }
        }
        .padding()
        .navigationTitle("接收文件")
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
}
