//
//  ThumbnailProvider.swift — Finder / Quick Look thumbnails for STEP files.
//
//  All the work happens in Rust (`sv_thumbnail_png`); this file only converts sizes, decodes the
//  PNG and draws it into the reply context.
//

import AppKit
import Foundation
import QuickLookThumbnailing
import StepViewFFI

/// Wall-clock budget handed to the renderer. Quick Look gives an extension a few seconds before it
/// gives up; staying well under that means a slow file degrades to a bounding-box silhouette
/// instead of producing nothing at all.
private let budgetMilliseconds: UInt32 = 900

/// Upper bound on the rendered image, independent of what Quick Look asks for.
private let maximumPixels: UInt32 = 1024

enum ThumbnailError: LocalizedError {
    case renderFailed(code: Int32, message: String)
    case decodeFailed

    var errorDescription: String? {
        switch self {
        case let .renderFailed(code, message):
            return message.isEmpty ? "step-ffi returned \(code)" : "\(message) (code \(code))"
        case .decodeFailed:
            return "the renderer returned bytes that are not a decodable PNG"
        }
    }
}

/// The last message `step-ffi` recorded on this thread.
private func lastFFIError() -> String {
    guard let c = sv_last_error() else { return "" }
    return String(cString: c)
}

/// Render `url` to a `CGImage`, or throw.
private func renderThumbnail(url: URL, pixels: UInt32) throws -> CGImage {
    var png: UnsafeMutablePointer<UInt8>?
    var length: Int = 0

    let code = url.withUnsafeFileSystemRepresentation { path -> Int32 in
        guard let path else { return SV_ERR_ARGS }
        return sv_thumbnail_png(path, pixels, budgetMilliseconds, &png, &length)
    }
    guard code >= 0, let png, length > 0 else {
        throw ThumbnailError.renderFailed(code: code, message: lastFFIError())
    }
    defer { sv_free_bytes(png, length) }

    // `Data(bytesNoCopy:)` would alias the Rust buffer past the `defer`; copy instead.
    let data = Data(bytes: png, count: length)
    guard let source = CGImageSourceCreateWithData(data as CFData, nil),
          let image = CGImageSourceCreateImageAtIndex(source, 0, nil)
    else {
        throw ThumbnailError.decodeFailed
    }
    return image
}

final class ThumbnailProvider: QLThumbnailProvider {
    override func provideThumbnail(
        for request: QLFileThumbnailRequest,
        _ handler: @escaping (QLThumbnailReply?, Error?) -> Void
    ) {
        // `maximumSize` is in points; the renderer wants pixels.
        let side = min(request.maximumSize.width, request.maximumSize.height) * request.scale
        let pixels = UInt32(max(16, min(Double(maximumPixels), side.rounded())))

        // Sandboxed extensions are handed access to the file, but ask explicitly in case the URL
        // arrives security-scoped (documents opened from another app's container).
        let scoped = request.fileURL.startAccessingSecurityScopedResource()
        defer { if scoped { request.fileURL.stopAccessingSecurityScopedResource() } }

        let image: CGImage
        do {
            image = try renderThumbnail(url: request.fileURL, pixels: pixels)
        } catch {
            // A nil reply tells Quick Look to fall back to the generic document icon.
            handler(nil, error)
            return
        }

        // The render is square; keep it square inside whatever box Quick Look offered.
        let points = min(request.maximumSize.width, request.maximumSize.height)
        let contextSize = CGSize(width: points, height: points)

        handler(QLThumbnailReply(contextSize: contextSize, currentContextDrawing: {
            guard let context = NSGraphicsContext.current?.cgContext else { return false }
            context.interpolationQuality = .high
            context.draw(image, in: CGRect(origin: .zero, size: contextSize))
            return true
        }), nil)
    }
}
