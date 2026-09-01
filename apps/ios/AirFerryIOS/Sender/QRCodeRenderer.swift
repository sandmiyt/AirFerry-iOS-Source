import CoreImage
import CoreImage.CIFilterBuiltins
import CoreGraphics
import Foundation

enum QRCodeRenderer {
    private static let context = CIContext(options: [.cacheIntermediates: false])

    static func image(for payload: Data) -> CGImage? {
        let filter = CIFilter.qrCodeGenerator()
        filter.message = payload
        filter.correctionLevel = "L"
        guard let output = filter.outputImage else { return nil }
        let extent = output.extent.integral
        return context.createCGImage(output, from: extent)
    }
}

