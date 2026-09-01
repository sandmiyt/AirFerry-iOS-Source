import Foundation

private struct SegmentAccumulator {
    let info: NativeSegmentInfo
    let directory: URL
    var received: Set<Int> = []

    mutating func store(index: Int, data: Data) throws {
        guard (0..<info.count).contains(index) else { throw NativeError.segment }
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let target = directory.appendingPathComponent(String(format: "segment-%06d.bin", index))
        try data.write(to: target, options: .atomic)
        received.insert(index)
    }

    var isComplete: Bool { received.count == info.count }

    func concatenate() throws -> URL {
        guard isComplete else { throw NativeError.segment }
        let output = directory.appendingPathComponent("root-stream.partial")
        FileManager.default.createFile(atPath: output.path, contents: nil)
        let writer = try FileHandle(forWritingTo: output)
        defer { try? writer.close() }
        for index in 0..<info.count {
            let part = directory.appendingPathComponent(String(format: "segment-%06d.bin", index))
            try writer.write(contentsOf: Data(contentsOf: part, options: .mappedIfSafe))
        }
        return output
    }
}

@MainActor
final class ReceiverViewModel: ObservableObject {
    @Published private(set) var progress = 0.0
    @Published private(set) var filename = "等待二维码…"
    @Published private(set) var status = "将发送端二维码完整放入取景框"
    @Published private(set) var receivedSegments = 0
    @Published private(set) var segmentCount = 1
    @Published private(set) var completedFile: ReceivedFile?
    @Published var previewFile: ReceivedFile?
    @Published var errorMessage: String?

    let camera = CameraScanner()
    private let native = NativeReceiver()
    private var accumulator: SegmentAccumulator?
    private var ignoredSessionKeys: Set<String> = []
    private var finishing = false

    init() {
        camera.onPayload = { [weak self] payload in
            Task { @MainActor in self?.ingest(payload) }
        }
    }

    func start() { camera.start() }
    func stop() { camera.stop() }

    func reset() {
        native.reset()
        if let accumulator { try? FileManager.default.removeItem(at: accumulator.directory) }
        accumulator = nil
        ignoredSessionKeys = []
        finishing = false
        progress = 0
        filename = "等待二维码…"
        status = "将发送端二维码完整放入取景框"
        receivedSegments = 0
        segmentCount = 1
        completedFile = nil
        previewFile = nil
        errorMessage = nil
        camera.start()
    }

    private func ingest(_ payload: Data) {
        guard !finishing, let sessionKey = frameSessionKey(payload), !ignoredSessionKeys.contains(sessionKey) else {
            return
        }
        do {
            let latest = try native.ingest(payload)
            progress = min(1, max(0, latest.decodedFraction))
            if latest.metaConfirmed {
                let learnedName = native.filename()
                if !learnedName.isEmpty { filename = learnedName }
                status = "正在恢复 · \(latest.receivedSymbols) / \(latest.totalSymbols) 个符号"
            } else {
                status = "已识别传输，等待元数据帧…"
            }
            if native.isComplete {
                finishing = true
                ignoredSessionKeys.insert(sessionKey)
                finishCurrentSession()
            }
        } catch NativeError.invalidFrame {
            // 相机画面中的普通二维码或模糊帧会落到这里，安静忽略即可。
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private func finishCurrentSession() {
        do {
            if native.isSegmented {
                let info = try native.segmentInfo()
                let raw = try native.assembleRaw()
                let cache = FileManager.default.temporaryDirectory
                    .appendingPathComponent("AirFerryIncoming", isDirectory: true)
                    .appendingPathComponent(info.rootKey, isDirectory: true)
                if accumulator == nil || accumulator?.info.rootKey != info.rootKey {
                    if let accumulator { try? FileManager.default.removeItem(at: accumulator.directory) }
                    accumulator = SegmentAccumulator(info: info, directory: cache)
                }
                try accumulator?.store(index: info.index, data: raw)
                receivedSegments = accumulator?.received.count ?? 0
                segmentCount = info.count
                native.reset()
                progress = 0
                if accumulator?.isComplete == true {
                    try finishAccumulator()
                } else {
                    status = "第 \(info.index + 1) 段完成，请在发送端切换下一段"
                    finishing = false
                }
            } else {
                let data = try native.assemble()
                let output = try FileLocations.uniqueOutputURL(filename: native.filename())
                try data.write(to: output, options: .atomic)
                native.reset()
                finish(output)
            }
        } catch {
            finishing = false
            errorMessage = error.localizedDescription
        }
    }

    private func finishAccumulator() throws {
        guard let accumulator else { throw NativeError.segment }
        status = "正在合并并校验文件…"
        let stream = try accumulator.concatenate()
        let output = try FileLocations.uniqueOutputURL(filename: accumulator.info.filename)
        try NativeReceiver.finishSegmented(input: stream, output: output, info: accumulator.info)
        try? FileManager.default.removeItem(at: accumulator.directory)
        self.accumulator = nil
        finish(output)
    }

    private func finish(_ output: URL) {
        camera.stop()
        progress = 1
        status = "接收完成，已保存并通过完整性校验"
        let file = ReceivedFile(url: output)
        completedFile = file
        previewFile = file
        finishing = false
    }

    private func frameSessionKey(_ data: Data) -> String? {
        guard data.count >= 20, data[0] == 0x45, data[1] == 0x54 else { return nil }
        return data[4..<20].map { String(format: "%02x", $0) }.joined()
    }
}
