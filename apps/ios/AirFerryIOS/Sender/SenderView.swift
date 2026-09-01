import SwiftUI
import UniformTypeIdentifiers

struct SenderView: View {
    @ObservedObject var model: SenderViewModel
    @State private var showingImporter = false

    var body: some View {
        Group {
            switch model.phase {
            case .idle, .staging, .ready:
                selectionView
            case .preparing:
                loadingView
            case .playing:
                playerView
            case .failed(let message):
                failureView(message)
            }
        }
        .navigationTitle("发送")
        .navigationBarTitleDisplayMode(.inline)
        .fileImporter(
            isPresented: $showingImporter,
            allowedContentTypes: [.item],
            allowsMultipleSelection: false
        ) { result in
            if case .success(let urls) = result, let url = urls.first {
                model.stage(url)
            }
        }
    }

    private var selectionView: some View {
        ZStack {
            AirFerryBackdrop()

            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    hero
                    AirFerryGlassGroup(spacing: 18) {
                        VStack(spacing: 18) {
                            fileCard
                            if !model.filename.isEmpty {
                                settingsCard
                            }
                        }
                    }

                    if !model.filename.isEmpty {
                        Button {
                            model.begin()
                        } label: {
                            Label("开始发送", systemImage: "paperplane.fill")
                                .font(.headline)
                                .frame(maxWidth: .infinity, minHeight: 54)
                        }
                        .airFerryPrimaryButton()
                    }

                    Label("完全离线 · 不需要互联网或局域网", systemImage: "wifi.slash")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity)
                }
                .frame(maxWidth: 680)
                .frame(maxWidth: .infinity)
                .padding(.horizontal, 20)
                .padding(.top, 18)
                .padding(.bottom, 32)
            }
        }
        .overlay {
            if model.phase == .staging {
                ZStack {
                    Color.black.opacity(0.12).ignoresSafeArea()
                    ProgressView("正在读取文件…")
                        .padding(.horizontal, 24)
                        .padding(.vertical, 18)
                        .airFerryGlassCard(cornerRadius: 20)
                }
            }
        }
    }

    private var hero: some View {
        VStack(alignment: .leading, spacing: 8) {
            ZStack {
                Circle().fill(AirFerryTheme.accent.gradient)
                Image(systemName: "point.3.connected.trianglepath.dotted")
                    .font(.system(size: 31, weight: .semibold))
                    .foregroundStyle(.white)
            }
            .frame(width: 62, height: 62)
            .shadow(color: AirFerryTheme.accent.opacity(0.25), radius: 18, y: 8)

            Text("隔空发送文件")
                .font(.largeTitle.bold())
            Text("选择文件后，用另一台设备的相机扫描动态二维码。")
                .font(.subheadline)
                .foregroundStyle(.secondary)
        }
        .padding(.vertical, 8)
    }

    private var fileCard: some View {
        Button {
            showingImporter = true
        } label: {
            HStack(spacing: 15) {
                Image(systemName: model.filename.isEmpty ? "doc.badge.plus" : "doc.fill")
                    .font(.system(size: 26, weight: .semibold))
                    .foregroundStyle(AirFerryTheme.accent)
                    .frame(width: 52, height: 52)
                    .background(AirFerryTheme.accent.opacity(0.10), in: RoundedRectangle(cornerRadius: 16))

                VStack(alignment: .leading, spacing: 4) {
                    Text(model.filename.isEmpty ? "选择文件" : model.filename)
                        .font(.headline)
                        .foregroundStyle(.primary)
                        .lineLimit(2)
                    Text(model.filename.isEmpty ? "支持照片、视频、音频和文档" : fileSizeText)
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }

                Spacer(minLength: 8)
                Image(systemName: "chevron.right")
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(.tertiary)
            }
            .padding(18)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .airFerryGlassCard(interactive: true)
    }

    private var settingsCard: some View {
        VStack(alignment: .leading, spacing: 18) {
            Label("传输设置", systemImage: "slider.horizontal.3")
                .font(.headline)

            VStack(alignment: .leading, spacing: 8) {
                Text("速度").font(.subheadline.weight(.medium))
                Picker("速度", selection: $model.speed) {
                    ForEach(SenderSpeed.allCases) { speed in
                        Text(speed.title).tag(speed)
                    }
                }
                .pickerStyle(.segmented)
            }

            VStack(alignment: .leading, spacing: 8) {
                Text("同屏二维码").font(.subheadline.weight(.medium))
                Picker("同屏二维码", selection: $model.codesPerTick) {
                    Text("单码 · 更稳").tag(1)
                    Text("双码 · 更快").tag(2)
                }
                .pickerStyle(.segmented)
            }

            Text("远距离、反光或相机对焦较慢时建议使用“高速 + 单码”。")
                .font(.footnote)
                .foregroundStyle(.secondary)
        }
        .padding(18)
        .airFerryGlassCard()
    }

    private var loadingView: some View {
        ZStack {
            AirFerryBackdrop()
            VStack(spacing: 16) {
                ProgressView().controlSize(.large)
                Text("正在生成传输任务…").font(.headline)
                Text("大文件首次准备需要一点时间")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }
            .padding(28)
            .airFerryGlassCard()
        }
    }

    private var playerView: some View {
        ZStack {
            AirFerryBackdrop()

            VStack(spacing: 14) {
                transmissionHeader
                QRStage(images: model.qrImages)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                if model.segmentCount > 1 {
                    segmentControls
                }
            }
            .frame(maxWidth: 980)
            .frame(maxWidth: .infinity)
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
        }
        .navigationTitle("正在发送")
        .toolbar(.hidden, for: .tabBar)
    }

    private var transmissionHeader: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 3) {
                Text(model.filename).font(.headline).lineLimit(1)
                Text("已播放 \(model.framesShown) 帧 · 本段约 \(model.totalSymbols) 个符号")
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 8)
            Button {
                model.reset()
            } label: {
                Label("重选", systemImage: "arrow.uturn.backward")
            }
            .airFerrySecondaryButton()
        }
        .padding(14)
        .airFerryGlassCard(cornerRadius: 22)
    }

    private var segmentControls: some View {
        VStack(spacing: 8) {
            AirFerryGlassGroup(spacing: 12) {
                HStack(spacing: 12) {
                    Button("上一段") { model.selectSegment(model.segmentIndex - 1) }
                        .disabled(model.segmentIndex == 0)
                        .airFerrySecondaryButton()
                    Text("第 \(model.segmentIndex + 1) / \(model.segmentCount) 段")
                        .font(.subheadline.monospacedDigit().weight(.semibold))
                    Button("下一段") { model.selectSegment(model.segmentIndex + 1) }
                        .disabled(model.segmentIndex + 1 >= model.segmentCount)
                        .airFerrySecondaryButton()
                }
            }
            Text("接收端提示本段完成后，再切换下一段")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }

    private func failureView(_ message: String) -> some View {
        ZStack {
            AirFerryBackdrop()
            ContentUnavailableView {
                Label("无法发送", systemImage: "exclamationmark.triangle")
            } description: {
                Text(message)
            } actions: {
                Button("返回重选") { model.reset() }
                    .airFerryPrimaryButton()
            }
        }
    }

    private var fileSizeText: String {
        ByteCountFormatter.string(fromByteCount: Int64(model.fileSize), countStyle: .file)
    }
}

private struct QRStage: View {
    let images: [CGImage]

    var body: some View {
        GeometryReader { proxy in
            let count = max(1, min(images.count, 2))
            let spacing: CGFloat = 10
            let totalSpacing = spacing * CGFloat(count - 1)
            let side = min((proxy.size.width - totalSpacing) / CGFloat(count), proxy.size.height)

            HStack(spacing: spacing) {
                ForEach(Array(images.prefix(2).enumerated()), id: \.offset) { _, image in
                    Image(decorative: image, scale: 1)
                        .interpolation(.none)
                        .resizable()
                        .scaledToFit()
                        .padding(max(8, side * 0.035))
                        .background(.white, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
                        .frame(width: side, height: side)
                        .shadow(color: .black.opacity(0.10), radius: 18, y: 8)
                }

                if images.isEmpty {
                    ProgressView()
                        .controlSize(.large)
                        .frame(width: side, height: side)
                }
            }
            .frame(width: proxy.size.width, height: proxy.size.height)
        }
    }
}
