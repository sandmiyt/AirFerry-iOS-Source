import CoreTransferable
import Foundation
import UniformTypeIdentifiers

struct PhotoPickerTransfer: Transferable {
    let url: URL

    static var transferRepresentation: some TransferRepresentation {
        FileRepresentation(importedContentType: .image) { received in
            try copyToTemporaryStorage(received.file)
        }
        FileRepresentation(importedContentType: .movie) { received in
            try copyToTemporaryStorage(received.file)
        }
    }

    private static func copyToTemporaryStorage(_ source: URL) throws -> PhotoPickerTransfer {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("AirFerryPhotoPicker", isDirectory: true)
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let filename = source.lastPathComponent.isEmpty ? "media" : source.lastPathComponent
        let destination = directory.appendingPathComponent(filename)
        try FileManager.default.copyItem(at: source, to: destination)
        return PhotoPickerTransfer(url: destination)
    }
}
