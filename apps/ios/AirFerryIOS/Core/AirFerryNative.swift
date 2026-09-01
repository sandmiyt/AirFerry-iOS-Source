import AirFerryCore
import Foundation

enum NativeError: LocalizedError {
    case createSender
    case invalidFrame
    case assemble
    case segment

    var errorDescription: String? {
        switch self {
        case .createSender: "无法创建发送任务，请确认文件不是空文件且小于 256 MB。"
        case .invalidFrame: "二维码数据不是有效的传输帧。"
        case .assemble: "文件恢复或完整性校验失败。"
        case .segment: "分段数据不完整或无法合并。"
        }
    }
}

final class NativeSender {
    private var handle: UnsafeMutableRawPointer?
    let frameCapacity: Int

    init(data: Data, filename: String, modifiedMilliseconds: UInt64,
         redundancy: UInt8 = 5, symbolSize: UInt32 = 1400) throws {
        let filenameData = Data(filename.utf8)
        let created = data.withUnsafeBytes { payload in
            filenameData.withUnsafeBytes { name in
                airferry_sender_create(
                    payload.bindMemory(to: UInt8.self).baseAddress,
                    data.count,
                    name.bindMemory(to: UInt8.self).baseAddress,
                    filenameData.count,
                    modifiedMilliseconds,
                    redundancy,
                    symbolSize
                )
            }
        }
        guard let created else { throw NativeError.createSender }
        handle = created
        frameCapacity = Int(airferry_sender_frame_capacity(created))
    }

    deinit {
        if let handle { airferry_sender_destroy(handle) }
    }

    var segmentCount: Int { Int(airferry_sender_segment_count(handle)) }
    var segmentIndex: Int { Int(airferry_sender_segment_index(handle)) }
    var totalSymbols: Int { Int(airferry_sender_total_symbols(handle)) }

    func selectSegment(_ index: Int) throws {
        guard airferry_sender_select_segment(handle, UInt32(index)) == 1 else {
            throw NativeError.segment
        }
    }

    func nextFrame() throws -> Data {
        var buffer = [UInt8](repeating: 0, count: frameCapacity)
        let written = buffer.withUnsafeMutableBufferPointer {
            airferry_sender_next_frame(handle, $0.baseAddress, $0.count)
        }
        guard written > 0 else { throw NativeError.invalidFrame }
        return Data(buffer.prefix(written))
    }
}

struct NativeProgress: Decodable {
    let totalSymbols: UInt32
    let receivedSymbols: UInt32
    let decodedFraction: Double
    let complete: Bool
    let metaConfirmed: Bool

    enum CodingKeys: String, CodingKey {
        case totalSymbols = "total_symbols"
        case receivedSymbols = "received_symbols"
        case decodedFraction = "decoded_fraction"
        case complete
        case metaConfirmed = "meta_confirmed"
    }
}

struct NativeSegmentInfo {
    let index: Int
    let count: Int
    let rootLow: UInt64
    let rootHigh: UInt64
    let compression: UInt8
    let expectedSize: UInt64
    let expectedCRC: UInt32
    let crcKnown: Bool
    let rootSHA256: Data
    let filename: String

    var rootKey: String { String(format: "%016llx%016llx", rootHigh, rootLow) }
}

final class NativeReceiver {
    private var handle: UnsafeMutableRawPointer?

    deinit { reset() }

    func reset() {
        if let handle { airferry_receiver_destroy(handle) }
        handle = nil
    }

    func ingest(_ frame: Data) throws -> NativeProgress {
        if handle == nil {
            handle = frame.withUnsafeBytes {
                airferry_receiver_create_from_frame(
                    $0.bindMemory(to: UInt8.self).baseAddress,
                    frame.count
                )
            }
        }
        guard let handle else { throw NativeError.invalidFrame }
        let status = frame.withUnsafeBytes {
            airferry_receiver_ingest(
                handle,
                $0.bindMemory(to: UInt8.self).baseAddress,
                frame.count
            )
        }
        guard UInt32(truncatingIfNeeded: status >> 32) != UInt32.max else {
            throw NativeError.invalidFrame
        }
        return try progress()
    }

    func progress() throws -> NativeProgress {
        guard let handle else { throw NativeError.invalidFrame }
        let needed = airferry_receiver_progress_json(handle, nil, 0)
        guard needed > 1 else { throw NativeError.invalidFrame }
        var bytes = [UInt8](repeating: 0, count: needed)
        _ = bytes.withUnsafeMutableBufferPointer {
            airferry_receiver_progress_json(handle, $0.baseAddress, $0.count)
        }
        return try JSONDecoder().decode(NativeProgress.self, from: Data(bytes.dropLast()))
    }

    var isComplete: Bool {
        guard let handle else { return false }
        return airferry_receiver_is_complete(handle) == 1
    }

    var isSegmented: Bool {
        guard let handle else { return false }
        return airferry_receiver_is_segmented(handle) == 1
    }

    func filename() -> String {
        guard let handle else { return "" }
        let needed = airferry_receiver_file_name(handle, nil, 0)
        guard needed > 1 else { return "" }
        var bytes = [UInt8](repeating: 0, count: needed)
        _ = bytes.withUnsafeMutableBufferPointer {
            airferry_receiver_file_name(handle, $0.baseAddress, $0.count)
        }
        return String(decoding: bytes.dropLast(), as: UTF8.self)
    }

    func assemble() throws -> Data {
        try copyRustBuffer(raw: false)
    }

    func assembleRaw() throws -> Data {
        try copyRustBuffer(raw: true)
    }

    private func copyRustBuffer(raw: Bool) throws -> Data {
        guard let handle else { throw NativeError.assemble }
        var pointer: UnsafeMutablePointer<UInt8>?
        var length = 0
        let ok = raw
            ? airferry_receiver_assemble_raw(handle, &pointer, &length)
            : airferry_receiver_assemble(handle, &pointer, &length)
        guard ok == 1, let pointer else { throw NativeError.assemble }
        defer { airferry_buffer_free(pointer, length) }
        return Data(bytes: pointer, count: length)
    }

    func segmentInfo() throws -> NativeSegmentInfo {
        guard let handle, isSegmented else { throw NativeError.segment }
        var sha = [UInt8](repeating: 0, count: 32)
        let written = sha.withUnsafeMutableBufferPointer {
            airferry_receiver_root_sha256(handle, $0.baseAddress, $0.count)
        }
        guard written == 32 else { throw NativeError.segment }
        return NativeSegmentInfo(
            index: Int(airferry_receiver_segment_index(handle)),
            count: Int(airferry_receiver_segment_count(handle)),
            rootLow: airferry_receiver_root_session_id_lo(handle),
            rootHigh: airferry_receiver_root_session_id_hi(handle),
            compression: airferry_receiver_compression(handle),
            expectedSize: airferry_receiver_original_size(handle),
            expectedCRC: UInt32(truncatingIfNeeded: airferry_receiver_crc32(handle)),
            crcKnown: airferry_receiver_crc32_known(handle) == 1,
            rootSHA256: Data(sha),
            filename: filename()
        )
    }

    static func finishSegmented(input: URL, output: URL, info: NativeSegmentInfo) throws {
        let sha = info.rootSHA256.map { String(format: "%02x", $0) }.joined()
        let ok = input.path.withCString { inputPath in
            output.path.withCString { outputPath in
                sha.withCString { shaCString in
                    airferry_decompress_stream_to_file(
                        inputPath,
                        outputPath,
                        info.compression,
                        info.expectedSize,
                        info.expectedSize,
                        info.expectedCRC,
                        info.crcKnown,
                        shaCString
                    )
                }
            }
        }
        guard ok == 1 else { throw NativeError.assemble }
    }
}

