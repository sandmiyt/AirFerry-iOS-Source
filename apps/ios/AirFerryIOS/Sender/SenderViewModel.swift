import Foundation
import CoreGraphics
import SwiftUI

enum SenderSpeed: String, CaseIterable, Identifiable {
    case stable
    case fast
    case aggressive

    var id: String { rawValue }
    var title: String {
        switch self {
        case .stable: "稳定"
        case .fast: "高速"
        case .aggressive: "激进"
        }
    }
    var symbolSize: UInt32 {
        switch self {
        case .stable: 512
        case .fast: 896
        case .aggressive: 1400
        }
    }
    var framesPerSecond: Int {
        switch self {
        case .stable: 15
        case .fast: 20
        case .aggressive: 24
        }
    }
}

private actor SenderEngine {
    private let native: NativeSender

    init(data: Data, filename: String, modifiedMilliseconds: UInt64, speed: SenderSpeed) throws {
        native = try NativeSender(
            data: data,
            filename: filename,
            modifiedMilliseconds: modifiedMilliseconds,
            redundancy: 5,
            symbolSize: speed.symbolSize
        )
    }

    func info() -> (segments: Int, index: Int, symbols: Int) {
        (native.segmentCount, native.segmentIndex, native.totalSymbols)
    }

    func nextFrames(count: Int) throws -> [Data] {
        try (0..<count).map { _ in try native.nextFrame() }
    }

    func selectSegment(_ index: Int) throws -> (segments: Int, index: Int, symbols: Int) {
        try native.selectSegment(index)
        return info()
    }
}

@MainActor
final class SenderViewModel: ObservableObject {
    enum Phase: Equatable {
        case idle
        case staging
        case ready
        case preparing
        case playing
        case failed(String)
    }

    @Published var phase: Phase = .idle
    @Published var speed: SenderSpeed = .fast
    @Published var codesPerTick = 1
    @Published private(set) var filename = ""
    @Published private(set) var fileSize: UInt64 = 0
    @Published private(set) var qrImages: [CGImage] = []
    @Published private(set) var segmentCount = 1
    @Published private(set) var segmentIndex = 0
    @Published private(set) var totalSymbols = 0
    @Published private(set) var framesShown: UInt64 = 0

    private var stagedURL: URL?
    private var modifiedMilliseconds: UInt64 = 0
    private var engine: SenderEngine?
    private var playTask: Task<Void, Never>?

    deinit { playTask?.cancel() }

    var isPlaying: Bool { phase == .playing }

    func stage(
        _ source: URL,
        preferredFilename: String? = nil,
        removeSourceAfterStaging: Bool = false
    ) {
        playTask?.cancel()
        phase = .staging
        Task {
            let scoped = source.startAccessingSecurityScopedResource()
            defer {
                if scoped { source.stopAccessingSecurityScopedResource() }
                if removeSourceAfterStaging {
                    try? FileManager.default.removeItem(at: source.deletingLastPathComponent())
                }
            }
            do {
                let values = try source.resourceValues(forKeys: [
                    .fileSizeKey,
                    .nameKey,
                    .contentModificationDateKey
                ])
                let directory = FileManager.default.temporaryDirectory
                    .appendingPathComponent("AirFerryOutgoing", isDirectory: true)
                try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
                let name = preferredFilename ?? values.name ?? source.lastPathComponent
                let destination = directory
                    .appendingPathComponent(UUID().uuidString, isDirectory: true)
                    .appendingPathComponent(name)
                try FileManager.default.createDirectory(
                    at: destination.deletingLastPathComponent(),
                    withIntermediateDirectories: true
                )
                try FileManager.default.copyItem(at: source, to: destination)
                stagedURL = destination
                filename = name
                fileSize = UInt64(values.fileSize ?? 0)
                let modified = values.contentModificationDate?.timeIntervalSince1970
                    ?? Date().timeIntervalSince1970
                modifiedMilliseconds = UInt64(max(0, modified * 1000))
                phase = .ready
            } catch {
                phase = .failed("读取文件失败：\(error.localizedDescription)")
            }
        }
    }

    func reportSelectionFailure(_ error: Error) {
        phase = .failed("导入所选内容失败：\(error.localizedDescription)")
    }

    func begin() {
        guard let stagedURL else { return }
        phase = .preparing
        qrImages = []
        Task {
            do {
                let url = stagedURL
                let selectedSpeed = speed
                let data = try await Task.detached(priority: .userInitiated) {
                    try Data(contentsOf: url, options: .mappedIfSafe)
                }.value
                let milliseconds = modifiedMilliseconds
                let newEngine = try await Task.detached(priority: .userInitiated) {
                    try SenderEngine(
                        data: data,
                        filename: url.lastPathComponent,
                        modifiedMilliseconds: milliseconds,
                        speed: selectedSpeed
                    )
                }.value
                engine = newEngine
                let info = await newEngine.info()
                apply(info)
                framesShown = 0
                phase = .playing
                startLoop()
            } catch {
                phase = .failed(error.localizedDescription)
            }
        }
    }

    func togglePlayback() {
        if playTask == nil {
            phase = .playing
            startLoop()
        } else {
            playTask?.cancel()
            playTask = nil
        }
    }

    func selectSegment(_ index: Int) {
        guard let engine, (0..<segmentCount).contains(index) else { return }
        playTask?.cancel()
        playTask = nil
        phase = .preparing
        Task {
            do {
                let info = try await engine.selectSegment(index)
                apply(info)
                framesShown = 0
                phase = .playing
                startLoop()
            } catch {
                phase = .failed(error.localizedDescription)
            }
        }
    }

    func reset() {
        playTask?.cancel()
        playTask = nil
        engine = nil
        if let stagedURL { try? FileManager.default.removeItem(at: stagedURL.deletingLastPathComponent()) }
        stagedURL = nil
        filename = ""
        fileSize = 0
        modifiedMilliseconds = 0
        qrImages = []
        segmentCount = 1
        segmentIndex = 0
        totalSymbols = 0
        framesShown = 0
        phase = .idle
    }

    private func startLoop() {
        guard let engine else { return }
        playTask?.cancel()
        let fps = speed.framesPerSecond
        playTask = Task { [weak self] in
            while !Task.isCancelled {
                guard let self else { return }
                do {
                    let frames = try await engine.nextFrames(count: codesPerTick)
                    let images = await Task.detached(priority: .userInitiated) {
                        frames.compactMap { QRCodeRenderer.image(for: $0) }
                    }.value
                    guard !Task.isCancelled else { return }
                    qrImages = images
                    framesShown += UInt64(frames.count)
                    try await Task.sleep(for: .milliseconds(1000 / fps))
                } catch is CancellationError {
                    return
                } catch {
                    phase = .failed(error.localizedDescription)
                    playTask = nil
                    return
                }
            }
        }
    }

    private func apply(_ info: (segments: Int, index: Int, symbols: Int)) {
        segmentCount = info.segments
        segmentIndex = info.index
        totalSymbols = info.symbols
    }
}
