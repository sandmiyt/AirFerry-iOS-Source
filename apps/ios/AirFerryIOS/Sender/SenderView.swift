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
                ProgressView("正在生成传输任务…")
            case .playing:
                playerView
            case .failed(let message):
                ContentUnavailableView {
                    Label("无法发送", systemImage: "exclamationmark.triangle")
                } description: {
                    Text(message)
                } actions: {
                    Button("返回重选") { model.reset() }
                }
            }
        }
        .navigationTitle("发送文件")
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
        Form {
            Section {
                Button {
                    showingImporter = true
                } label: {
                    Label(model.filename.isEmpty ? "选择文件" : "重新选择", systemImage: "doc.badge.plus")
                }
                if !model.filename.isEmpty {
                    LabeledContent("文件", value: model.filename)
                    LabeledContent("大小", value: ByteCountFormatter.string(fromByteCount: Int64(model.fileSize), countStyle: .file))
                }
            }

            if !model.filename.isEmpty {
                Section("传输设置") {
                    Picker("速度", selection: $model.speed) {
                        ForEach(SenderSpeed.allCases) { speed in
                            Text(speed.title).tag(speed)
                        }
                    }
                    Picker("同屏二维码", selection: $model.codesPerTick) {
                        Text("单码（更稳）").tag(1)
                        Text("双码（更快）").tag(2)
                    }
                    Text("激进模式每帧数据更多；扫码距离较远或屏幕反光时改用高速/稳定。双码需要接收端同时看清两个二维码。")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }

                Section {
                    Button("开始发送") { model.begin() }
                        .frame(maxWidth: .infinity)
                        .buttonStyle(.borderedProminent)
                }
            }

            Section("离线说明") {
                Text("发送与接收都只通过屏幕和相机，不需要互联网，也不需要两台设备处于同一局域网。")
            }
        }
        .overlay {
            if model.phase == .staging {
                ZStack {
                    Color.black.opacity(0.12).ignoresSafeArea()
                    ProgressView("正在读取文件…")
                        .padding()
                        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 16))
                }
            }
        }
    }

    private var playerView: some View {
        VStack(spacing: 12) {
            HStack {
                VStack(alignment: .leading, spacing: 3) {
                    Text(model.filename).font(.headline).lineLimit(1)
                    Text("已播放 \(model.framesShown) 帧 · 本段约 \(model.totalSymbols) 个源符号")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer()
                Button("返回重选") { model.reset() }
                    .buttonStyle(.bordered)
            }

            GeometryReader { proxy in
                let columns = Array(repeating: GridItem(.flexible(), spacing: 8), count: model.qrImages.count > 1 ? 2 : 1)
                LazyVGrid(columns: columns, spacing: 8) {
                    ForEach(Array(model.qrImages.enumerated()), id: \.offset) { _, image in
                        Image(decorative: image, scale: 1)
                            .interpolation(.none)
                            .resizable()
                            .scaledToFit()
                            .padding(10)
                            .background(.white)
                            .clipShape(RoundedRectangle(cornerRadius: 12))
                    }
                }
                .frame(width: proxy.size.width, height: proxy.size.height)
            }

            if model.segmentCount > 1 {
                HStack {
                    Button("上一段") { model.selectSegment(model.segmentIndex - 1) }
                        .disabled(model.segmentIndex == 0)
                    Text("第 \(model.segmentIndex + 1) / \(model.segmentCount) 段")
                        .font(.subheadline.monospacedDigit())
                    Button("下一段") { model.selectSegment(model.segmentIndex + 1) }
                        .disabled(model.segmentIndex + 1 >= model.segmentCount)
                }
                Text("接收端提示本段完成后，再切换下一段。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .padding()
        .toolbar(.hidden, for: .tabBar)
        .statusBarHidden()
    }
}

