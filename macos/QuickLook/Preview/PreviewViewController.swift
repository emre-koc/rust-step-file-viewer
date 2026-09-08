//
//  PreviewViewController.swift — Space-bar (Quick Look) preview for STEP files.
//
//  `sv_load` hands over one buffer set per (shape, body) plus one placement per (assembly instance
//  × body). Each mesh becomes a single SCNGeometry that every placement of that part shares, so an
//  assembly with a hundred identical screws uploads one screw.
//

import Cocoa
import Foundation
import Quartz
import SceneKit
import StepViewFFI
import simd

/// Tessellation quality: 1 = "preview", the same setting the StepView app uses, so a file the user
/// has already opened is served straight from the app's warm mesh cache.
private let loadQuality: UInt32 = 1

/// The model is rescaled so its bounding-box diagonal is this many SceneKit units, which keeps one
/// set of camera clip planes usable for a 5 mm bracket and a 5 m weldment alike.
private let sceneDiagonal: Float = 10

private let backgroundColor = NSColor(calibratedWhite: 0.16, alpha: 1)
private let edgeColor = NSColor(calibratedWhite: 0.08, alpha: 1)

enum PreviewError: LocalizedError {
    case loadFailed(String)

    var errorDescription: String? {
        switch self {
        case let .loadFailed(message):
            return message.isEmpty ? "the file could not be loaded" : message
        }
    }
}

private func lastFFIError() -> String {
    guard let c = sv_last_error() else { return "" }
    return String(cString: c)
}

// MARK: - Model → SceneKit

/// Everything pulled out of one `sv_model`, ready to hang off a scene graph.
struct LoadedModel {
    var geometries: [SCNGeometry]
    /// `(geometry index, transform, name)` per placement.
    var placements: [(mesh: Int, transform: simd_float4x4, name: String)]
    var center: SIMD3<Float>
    var diagonal: Float
}

private func makeGeometry(_ model: OpaquePointer, mesh: UInt32) -> SCNGeometry? {
    var vertexCount: UInt32 = 0
    var indexCount: UInt32 = 0
    var edgeCount: UInt32 = 0
    var doubleSided: UInt8 = 0
    guard sv_mesh_info(model, mesh, &vertexCount, &indexCount, &edgeCount, &doubleSided) == SV_OK,
          vertexCount > 0, indexCount >= 3
    else { return nil }

    let vc = Int(vertexCount)
    var positions = [Float](repeating: 0, count: vc * 3)
    var normals = [Float](repeating: 0, count: vc * 3)
    var colors = [UInt8](repeating: 0, count: vc * 4)
    var indices = [UInt32](repeating: 0, count: Int(indexCount))
    var edges = [UInt32](repeating: 0, count: Int(edgeCount))

    let ok = positions.withUnsafeMutableBufferPointer { p in
        normals.withUnsafeMutableBufferPointer { n in
            colors.withUnsafeMutableBufferPointer { c in
                indices.withUnsafeMutableBufferPointer { i in
                    edges.withUnsafeMutableBufferPointer { e in
                        sv_mesh_buffers(model, mesh, p.baseAddress, n.baseAddress, c.baseAddress,
                                        i.baseAddress, e.isEmpty ? nil : e.baseAddress)
                    }
                }
            }
        }
    }
    guard ok == SV_OK else { return nil }

    let vertexSource = SCNGeometrySource(
        data: positions.withUnsafeBufferPointer { Data(buffer: $0) },
        semantic: .vertex, vectorCount: vc, usesFloatComponents: true, componentsPerVector: 3,
        bytesPerComponent: MemoryLayout<Float>.size, dataOffset: 0,
        dataStride: MemoryLayout<Float>.size * 3)
    let normalSource = SCNGeometrySource(
        data: normals.withUnsafeBufferPointer { Data(buffer: $0) },
        semantic: .normal, vectorCount: vc, usesFloatComponents: true, componentsPerVector: 3,
        bytesPerComponent: MemoryLayout<Float>.size, dataOffset: 0,
        dataStride: MemoryLayout<Float>.size * 3)
    // Per-vertex B-rep face colour. The FFI hands these over as RGBA8, but SceneKit does *not*
    // normalize integer colour components — feeding it bytes renders every channel >= 1 clamped
    // (grey turns white, orange turns yellow) and the lighting stops reading. So widen to floats.
    let rgba = colors.map { Float($0) / 255 }
    let colorSource = SCNGeometrySource(
        data: rgba.withUnsafeBufferPointer { Data(buffer: $0) },
        semantic: .color, vectorCount: vc, usesFloatComponents: true, componentsPerVector: 4,
        bytesPerComponent: MemoryLayout<Float>.size, dataOffset: 0,
        dataStride: MemoryLayout<Float>.size * 4)

    var elements: [SCNGeometryElement] = [
        SCNGeometryElement(data: indices.withUnsafeBufferPointer { Data(buffer: $0) },
                           primitiveType: .triangles, primitiveCount: Int(indexCount) / 3,
                           bytesPerIndex: MemoryLayout<UInt32>.size)
    ]
    if edgeCount >= 2 {
        elements.append(SCNGeometryElement(data: edges.withUnsafeBufferPointer { Data(buffer: $0) },
                                           primitiveType: .line, primitiveCount: Int(edgeCount) / 2,
                                           bytesPerIndex: MemoryLayout<UInt32>.size))
    }

    let geometry = SCNGeometry(sources: [vertexSource, normalSource, colorSource], elements: elements)

    // Materials map onto elements in order: shaded triangles first, feature edges second.
    let surface = SCNMaterial()
    surface.lightingModel = .blinn
    surface.diffuse.contents = NSColor.white   // modulated by the per-vertex colour source
    surface.specular.contents = NSColor(calibratedWhite: 0.35, alpha: 1)
    surface.shininess = 0.35
    surface.isDoubleSided = doubleSided != 0
    var materials = [surface]
    if elements.count > 1 {
        let line = SCNMaterial()
        line.lightingModel = .constant
        line.diffuse.contents = edgeColor      // black × any vertex colour is still black
        line.isDoubleSided = true
        materials.append(line)
    }
    geometry.materials = materials
    return geometry
}

func loadModel(url: URL) throws -> LoadedModel {
    let handle: OpaquePointer? = url.withUnsafeFileSystemRepresentation { path in
        guard let path else { return nil }
        return sv_load(path, loadQuality)
    }
    guard let model = handle else { throw PreviewError.loadFailed(lastFFIError()) }
    defer { sv_free(model) }

    let meshCount = sv_mesh_count(model)
    guard meshCount > 0 else { throw PreviewError.loadFailed("the file holds no drawable geometry") }

    // A geometry index of -1 marks a mesh that failed to build, so placements can skip it.
    var geometries: [SCNGeometry] = []
    var remap = [Int](repeating: -1, count: Int(meshCount))
    for mesh in 0..<meshCount {
        if let g = makeGeometry(model, mesh: mesh) {
            remap[Int(mesh)] = geometries.count
            geometries.append(g)
        }
    }
    guard !geometries.isEmpty else { throw PreviewError.loadFailed("no mesh could be built") }

    var placements: [(mesh: Int, transform: simd_float4x4, name: String)] = []
    var name = [CChar](repeating: 0, count: 256)
    for i in 0..<sv_instance_count(model) {
        var mesh: UInt32 = 0
        var m = [Float](repeating: 0, count: 16)
        let ok = m.withUnsafeMutableBufferPointer { buf in
            name.withUnsafeMutableBufferPointer { n in
                sv_instance(model, i, &mesh, buf.baseAddress, n.baseAddress, n.count)
            }
        }
        guard ok == SV_OK, Int(mesh) < remap.count, remap[Int(mesh)] >= 0 else { continue }
        let transform = simd_float4x4(columns: (SIMD4(m[0], m[1], m[2], m[3]),
                                                SIMD4(m[4], m[5], m[6], m[7]),
                                                SIMD4(m[8], m[9], m[10], m[11]),
                                                SIMD4(m[12], m[13], m[14], m[15])))
        placements.append((remap[Int(mesh)], transform, String(cString: name)))
    }
    guard !placements.isEmpty else { throw PreviewError.loadFailed("no instance references a mesh") }

    var bbox = [Double](repeating: 0, count: 6)
    sv_model_bbox(model, &bbox)
    let lo = SIMD3<Float>(Float(bbox[0]), Float(bbox[1]), Float(bbox[2]))
    let hi = SIMD3<Float>(Float(bbox[3]), Float(bbox[4]), Float(bbox[5]))
    let diagonal = max(simd_length(hi - lo), 1e-4)

    return LoadedModel(geometries: geometries, placements: placements,
                       center: (lo + hi) * 0.5, diagonal: diagonal)
}

// MARK: - View controller

class PreviewViewController: NSViewController, QLPreviewingController {
    private var sceneView: SCNView!

    override func loadView() {
        let view = SCNView(frame: NSRect(x: 0, y: 0, width: 800, height: 600))
        view.allowsCameraControl = true
        // Lighting is our own three-point rig (see `addLights`), not SceneKit's default.
        view.autoenablesDefaultLighting = false
        view.antialiasingMode = .multisampling4X
        view.backgroundColor = backgroundColor
        view.autoresizingMask = [.width, .height]
        view.scene = SCNScene()
        sceneView = view
        self.view = view
    }

    func preparePreviewOfFile(at url: URL, completionHandler handler: @escaping (Error?) -> Void) {
        let scoped = url.startAccessingSecurityScopedResource()
        DispatchQueue.global(qos: .userInitiated).async {
            let result = Result { try loadModel(url: url) }
            if scoped { url.stopAccessingSecurityScopedResource() }
            DispatchQueue.main.async {
                switch result {
                case let .success(model):
                    self.build(model)
                    handler(nil)
                case let .failure(error):
                    handler(error)
                }
            }
        }
    }

    private func build(_ model: LoadedModel) {
        let (scene, camera) = makePreviewScene(model)
        sceneView.scene = scene
        sceneView.pointOfView = camera
    }
}

// MARK: - Scene assembly

/// Assemble the scene graph and return it with the camera to look through:
///
///     root → fit (scale + recentre) → zUp (CAD Z-up → SceneKit Y-up) → one node per placement
///
/// Split out of the view controller so the same scene can be rendered offscreen by a test harness.
func makePreviewScene(_ model: LoadedModel) -> (SCNScene, SCNNode) {
    let scene = SCNScene()
    scene.background.contents = backgroundColor

    let parts = SCNNode()
    for placement in model.placements {
        let node = SCNNode(geometry: model.geometries[placement.mesh])
        node.simdTransform = placement.transform
        node.name = placement.name.isEmpty ? nil : placement.name
        parts.addChildNode(node)
    }
    // Millimetres, still Z-up, still wherever the file put the model.
    parts.simdPosition = -model.center

    let zUp = SCNNode()
    zUp.eulerAngles = SCNVector3(-Double.pi / 2, 0, 0)
    zUp.addChildNode(parts)

    let fit = SCNNode()
    let s = sceneDiagonal / model.diagonal
    fit.simdScale = SIMD3(repeating: s)
    fit.addChildNode(zUp)
    scene.rootNode.addChildNode(fit)

    // Standard CAD isometric: after the Z-up fix the model's (+X, +Y, +Z) are SceneKit's
    // (+X, back, +Y), so the usual (1, 1, 1) eye lands on the corner the thumbnail shows.
    let camera = SCNCamera()
    camera.zNear = Double(sceneDiagonal) * 0.02
    camera.zFar = Double(sceneDiagonal) * 20
    camera.fieldOfView = 40
    let cameraNode = SCNNode()
    cameraNode.camera = camera
    let distance = sceneDiagonal * 0.5 / Float(sin(20 * Double.pi / 180))
    cameraNode.simdPosition = simd_normalize(SIMD3<Float>(1, 1, 1)) * distance
    cameraNode.look(at: SCNVector3(0, 0, 0))
    scene.rootNode.addChildNode(cameraNode)

    addLights(to: scene, camera: cameraNode)
    return (scene, cameraNode)
}

/// A three-point rig carried by the camera, so orbiting never swings the model into the dark.
///
/// `autoenablesDefaultLighting` is deliberately *not* used: its single 1000-lumen omni blows out
/// CAD colours (light grey clips to white, orange clips to yellow) and with everything clipped the
/// shape stops reading. These intensities sum to roughly one full exposure instead.
private func addLights(to scene: SCNScene, camera: SCNNode) {
    func directional(_ intensity: CGFloat, _ pitch: Float, _ yaw: Float) -> SCNNode {
        let light = SCNLight()
        light.type = .directional
        light.intensity = intensity
        light.castsShadow = false
        let node = SCNNode()
        node.light = light
        node.eulerAngles = SCNVector3(Double(pitch), Double(yaw), 0)
        return node
    }
    let ambient = SCNLight()
    ambient.type = .ambient
    ambient.intensity = 330
    let ambientNode = SCNNode()
    ambientNode.light = ambient
    scene.rootNode.addChildNode(ambientNode)

    camera.addChildNode(directional(560, -0.35, 0.45))   // key, up and to the right of the eye
    camera.addChildNode(directional(200, 0.55, -0.75))   // fill, below and to the left
}
