import Foundation

struct ReceivedFile: Identifiable, Equatable {
    let url: URL
    var id: String { url.path }
}

enum FileLocations {
    static func receivedDirectory() throws -> URL {
        let root = try FileManager.default.url(
            for: .documentDirectory,
            in: .userDomainMask,
            appropriateFor: nil,
            create: true
        )
        let directory = root.appendingPathComponent("已接收", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        return directory
    }

    static func uniqueOutputURL(filename: String) throws -> URL {
        let safe = filename
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: ":", with: "_")
        let base = safe.isEmpty ? "接收文件" : safe
        let directory = try receivedDirectory()
        var candidate = directory.appendingPathComponent(base)
        var suffix = 1
        while FileManager.default.fileExists(atPath: candidate.path) {
            let ext = (base as NSString).pathExtension
            let stem = (base as NSString).deletingPathExtension
            let name = ext.isEmpty ? "\(stem) (\(suffix))" : "\(stem) (\(suffix)).\(ext)"
            candidate = directory.appendingPathComponent(name)
            suffix += 1
        }
        return candidate
    }
}

