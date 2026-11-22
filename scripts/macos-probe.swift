import AppKit
import ApplicationServices
import Carbon
import Foundation
import ImageIO
import Metal

final class Counters: @unchecked Sendable {
    private let lock = NSLock()
    private var values: [String: Int] = [:]
    private var notes: [String] = []

    func bump(_ key: String) {
        lock.lock()
        values[key, default: 0] += 1
        lock.unlock()
    }

    func note(_ text: String) {
        lock.lock()
        notes.append(text)
        lock.unlock()
    }

    func count(_ key: String) -> Int {
        lock.lock()
        defer { lock.unlock() }
        return values[key, default: 0]
    }

    func reset() {
        lock.lock()
        values = [:]
        lock.unlock()
    }

    func recorded() -> [String] {
        lock.lock()
        defer { lock.unlock() }
        return notes
    }
}

let counters = Counters()

final class KeyView: NSView {
    override var acceptsFirstResponder: Bool { true }

    override func keyDown(with event: NSEvent) {
        counters.bump("view_key_down")
    }
}

func emit(_ ok: Bool, _ name: String, _ detail: String, required: Bool = false) {
    let clean = detail.replacingOccurrences(of: "\t", with: " ").replacingOccurrences(of: "\n", with: " ")
    print("\(ok ? "ok" : "missing")\t\(name)\t\(clean)\t\(required ? "required" : "fact")")
    fflush(stdout)
}

func launch() -> NSApplication {
    let app = NSApplication.shared
    app.setActivationPolicy(.regular)
    app.finishLaunching()
    return app
}

func pump(_ app: NSApplication, _ seconds: Double, until done: () -> Bool = { false }) {
    let deadline = Date().addingTimeInterval(seconds)
    while Date() < deadline && !done() {
        if let event = app.nextEvent(matching: .any, until: Date().addingTimeInterval(0.02), inMode: .default, dequeue: true) {
            app.sendEvent(event)
        }
    }
}

func run(_ app: NSApplication, _ path: String, _ arguments: [String], timeout: Double) -> (status: Int32?, output: String) {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: path)
    process.arguments = arguments
    let pipe = Pipe()
    process.standardOutput = pipe
    process.standardError = pipe
    do {
        try process.run()
    } catch {
        return (nil, "launch failed: \(error)")
    }
    pump(app, timeout, until: { !process.isRunning })
    if process.isRunning {
        process.terminate()
        process.waitUntilExit()
        return (nil, "timed out after \(timeout) s")
    }
    let data = pipe.fileHandleForReading.readDataToEndOfFile()
    return (process.terminationStatus, String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines))
}

let shaderSource = """
#include <metal_stdlib>
using namespace metal;

struct Varyings {
    float4 position [[position]];
};

vertex Varyings vs_full(uint id [[vertex_id]]) {
    float2 corner = float2(float((id << 1) & 2), float(id & 2));
    Varyings out;
    out.position = float4(corner * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

struct DualOut {
    float4 color [[color(0), index(0)]];
    float4 blend [[color(0), index(1)]];
};

fragment DualOut ps_dual(Varyings in [[stage_in]]) {
    DualOut out;
    out.color = float4(0.25, 0.5, 0.0, 1.0);
    out.blend = float4(0.5, 0.0, 1.0, 1.0);
    return out;
}

kernel void cs_fill(texture2d<half, access::write> target [[texture(0)]], uint2 gid [[thread_position_in_grid]]) {
    target.write(half4(0.25h, 0.5h, 0.75h, 1.0h), gid);
}

kernel void cs_accumulate(texture2d<half, access::read_write> target [[texture(0)]], uint2 gid [[thread_position_in_grid]]) {
    target.write(target.read(gid) + half4(0.25h), gid);
}
"""

let side = 8

func readback(_ device: MTLDevice, _ queue: MTLCommandQueue, _ texture: MTLTexture, bytesPerPixel: Int, encode: (MTLCommandBuffer) -> Void) -> (bytes: [UInt8]?, error: String?, gpuMilliseconds: Double) {
    let length = side * side * bytesPerPixel
    guard let buffer = device.makeBuffer(length: length, options: .storageModeShared),
          let commands = queue.makeCommandBuffer() else {
        return (nil, "no buffer or command buffer", 0)
    }
    encode(commands)
    guard let blit = commands.makeBlitCommandEncoder() else {
        return (nil, "no blit encoder", 0)
    }
    blit.copy(from: texture, sourceSlice: 0, sourceLevel: 0, sourceOrigin: MTLOrigin(x: 0, y: 0, z: 0),
              sourceSize: MTLSize(width: side, height: side, depth: 1), to: buffer, destinationOffset: 0,
              destinationBytesPerRow: side * bytesPerPixel, destinationBytesPerImage: length)
    blit.endEncoding()
    commands.commit()
    commands.waitUntilCompleted()
    guard commands.status == .completed else {
        return (nil, "command buffer status \(commands.status.rawValue): \(String(describing: commands.error))", 0)
    }
    let pointer = buffer.contents().bindMemory(to: UInt8.self, capacity: length)
    let bytes = Array(UnsafeBufferPointer(start: pointer, count: length))
    return (bytes, nil, (commands.gpuEndTime - commands.gpuStartTime) * 1000)
}

func halves(_ bytes: [UInt8], at pixel: Int) -> [Double] {
    (0..<4).map { channel in
        let offset = pixel * 8 + channel * 2
        let bits = UInt16(bytes[offset]) | (UInt16(bytes[offset + 1]) << 8)
        return Double(Float(Float16(bitPattern: bits)))
    }
}

func uniform(_ bytes: [UInt8], stride: Int) -> Bool {
    let first = Array(bytes[0..<stride])
    return (0..<(side * side)).allSatisfy { Array(bytes[($0 * stride)..<($0 * stride + stride)]) == first }
}

func metalFacts(_ artifacts: URL) -> [String: Any] {
    var facts: [String: Any] = [:]
    let all = MTLCopyAllDevices().map { $0.name }
    facts["all_devices"] = all
    guard let device = MTLCreateSystemDefaultDevice() else {
        facts["device"] = NSNull()
        emit(false, "Metal device", "MTLCreateSystemDefaultDevice returned nil (MTLCopyAllDevices: \(all))", required: true)
        return facts
    }
    facts["device"] = device.name
    let families: [(String, MTLGPUFamily)] = [
        ("apple5", .apple5), ("apple6", .apple6), ("apple7", .apple7), ("apple8", .apple8), ("apple9", .apple9),
        ("mac2", .mac2), ("metal3", .metal3), ("common3", .common3),
    ]
    let supported = Dictionary(uniqueKeysWithValues: families.map { ($0.0, device.supportsFamily($0.1)) })
    facts["families"] = supported
    facts["recommended_max_working_set_size"] = device.recommendedMaxWorkingSetSize
    facts["read_write_texture_tier"] = device.readWriteTextureSupport.rawValue
    facts["argument_buffers_tier"] = device.argumentBuffersSupport.rawValue
    facts["has_unified_memory"] = device.hasUnifiedMemory
    facts["is_headless"] = device.isHeadless
    facts["is_low_power"] = device.isLowPower
    facts["location"] = device.location.rawValue
    facts["max_buffer_length"] = device.maxBufferLength
    facts["max_threads_per_threadgroup"] = [device.maxThreadsPerThreadgroup.width, device.maxThreadsPerThreadgroup.height, device.maxThreadsPerThreadgroup.depth]
    facts["supports_32bit_float_filtering"] = device.supports32BitFloatFiltering
    facts["supports_bc_texture_compression"] = device.supportsBCTextureCompression
    let sampling: [(String, MTLCounterSamplingPoint)] = [
        ("stage", .atStageBoundary), ("draw", .atDrawBoundary), ("dispatch", .atDispatchBoundary),
        ("blit", .atBlitBoundary), ("tile_dispatch", .atTileDispatchBoundary),
    ]
    facts["counter_sampling"] = Dictionary(uniqueKeysWithValues: sampling.map { ($0.0, device.supportsCounterSampling($0.1)) })
    facts["counter_sets"] = device.counterSets?.map { $0.name } ?? []
    facts["sample_timestamps_gpu_nonzero"] = device.sampleTimestamps().gpu != 0
    let familyList = families.filter { supported[$0.0] == true }.map { $0.0 }.joined(separator: " ")
    emit(true, "Metal device", "\(device.name), families [\(familyList)], working set \(device.recommendedMaxWorkingSetSize / 1_048_576) MiB, rw tier \(device.readWriteTextureSupport.rawValue), stage counters \(device.supportsCounterSampling(.atStageBoundary)), all devices \(all)", required: true)

    var versions: [(String, MTLLanguageVersion)] = [("2.3", .version2_3), ("2.4", .version2_4), ("3.0", .version3_0), ("3.1", .version3_1)]
    if #available(macOS 15.0, *) {
        versions.append(("3.2", .version3_2))
    }
    var accepted: [String: String] = [:]
    for (name, version) in versions {
        let options = MTLCompileOptions()
        options.languageVersion = version
        do {
            _ = try device.makeLibrary(source: shaderSource, options: options)
            accepted[name] = "ok"
        } catch {
            accepted[name] = "\(error)"
        }
    }
    facts["msl_versions"] = accepted
    emit(accepted.values.allSatisfy { $0 == "ok" }, "MSL language versions", accepted.keys.sorted().map { "\($0)=\(accepted[$0] == "ok" ? "ok" : "fail")" }.joined(separator: " "))

    guard let queue = device.makeCommandQueue() else {
        emit(false, "Metal command queue", "makeCommandQueue returned nil", required: true)
        return facts
    }
    let library: MTLLibrary
    do {
        library = try device.makeLibrary(source: shaderSource, options: nil)
    } catch {
        emit(false, "MSL source library", "\(error)", required: true)
        return facts
    }

    let pipeline = MTLRenderPipelineDescriptor()
    pipeline.vertexFunction = library.makeFunction(name: "vs_full")
    pipeline.fragmentFunction = library.makeFunction(name: "ps_dual")
    let attachment = pipeline.colorAttachments[0]!
    attachment.pixelFormat = .rgba8Unorm
    attachment.isBlendingEnabled = true
    attachment.rgbBlendOperation = .add
    attachment.alphaBlendOperation = .add
    attachment.sourceRGBBlendFactor = .one
    attachment.destinationRGBBlendFactor = .oneMinusSource1Color
    attachment.sourceAlphaBlendFactor = .one
    attachment.destinationAlphaBlendFactor = .zero
    var dual: [String: Any] = ["expected": [128, 255, 0, 255]]
    do {
        let state = try device.makeRenderPipelineState(descriptor: pipeline)
        let descriptor = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rgba8Unorm, width: side, height: side, mipmapped: false)
        descriptor.usage = [.renderTarget]
        descriptor.storageMode = .private
        let target = device.makeTexture(descriptor: descriptor)!
        let result = readback(device, queue, target, bytesPerPixel: 4) { commands in
            let pass = MTLRenderPassDescriptor()
            pass.colorAttachments[0].texture = target
            pass.colorAttachments[0].loadAction = .clear
            pass.colorAttachments[0].clearColor = MTLClearColor(red: 0.5, green: 0.5, blue: 0.5, alpha: 1)
            pass.colorAttachments[0].storeAction = .store
            let encoder = commands.makeRenderCommandEncoder(descriptor: pass)!
            encoder.setRenderPipelineState(state)
            encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 3)
            encoder.endEncoding()
        }
        if let bytes = result.bytes {
            let pixel = Array(bytes[0..<4]).map { Int($0) }
            let matches = zip(pixel, [128, 255, 0, 255]).allSatisfy { abs($0 - $1) <= 1 } && uniform(bytes, stride: 4)
            dual["pixel"] = pixel
            dual["matches"] = matches
            dual["gpu_ms"] = result.gpuMilliseconds
            emit(matches, "dual-source blending", "pixel \(pixel), expected [128, 255, 0, 255], gpu \(String(format: "%.3f", result.gpuMilliseconds)) ms", required: true)
        } else {
            dual["error"] = result.error ?? "unknown"
            emit(false, "dual-source blending", result.error ?? "unknown", required: true)
        }
    } catch {
        dual["error"] = "\(error)"
        emit(false, "dual-source blending", "pipeline: \(error)", required: true)
    }
    facts["dual_source"] = dual

    var compute: [String: Any] = ["expected": [0.25, 0.5, 0.75, 1.0]]
    do {
        let fill = try device.makeComputePipelineState(function: library.makeFunction(name: "cs_fill")!)
        let descriptor = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rgba16Float, width: side, height: side, mipmapped: false)
        descriptor.usage = [.shaderRead, .shaderWrite]
        descriptor.storageMode = .private
        let target = device.makeTexture(descriptor: descriptor)!
        let groups = MTLSize(width: 1, height: 1, depth: 1)
        let threads = MTLSize(width: side, height: side, depth: 1)
        let result = readback(device, queue, target, bytesPerPixel: 8) { commands in
            let encoder = commands.makeComputeCommandEncoder()!
            encoder.setComputePipelineState(fill)
            encoder.setTexture(target, index: 0)
            encoder.dispatchThreadgroups(groups, threadsPerThreadgroup: threads)
            encoder.endEncoding()
        }
        if let bytes = result.bytes {
            let value = halves(bytes, at: 0)
            let matches = value == [0.25, 0.5, 0.75, 1.0] && uniform(bytes, stride: 8)
            compute["value"] = value
            compute["matches"] = matches
            emit(matches, "rgba16f compute write", "value \(value), expected [0.25, 0.5, 0.75, 1.0]", required: true)
        } else {
            compute["error"] = result.error ?? "unknown"
            emit(false, "rgba16f compute write", result.error ?? "unknown", required: true)
        }
        do {
            let accumulate = try device.makeComputePipelineState(function: library.makeFunction(name: "cs_accumulate")!)
            let second = readback(device, queue, target, bytesPerPixel: 8) { commands in
                let encoder = commands.makeComputeCommandEncoder()!
                encoder.setComputePipelineState(accumulate)
                encoder.setTexture(target, index: 0)
                encoder.dispatchThreadgroups(groups, threadsPerThreadgroup: threads)
                encoder.endEncoding()
            }
            if let bytes = second.bytes {
                let value = halves(bytes, at: 0)
                compute["read_write_value"] = value
                compute["read_write_matches"] = value == [0.5, 0.75, 1.0, 1.25]
                emit(value == [0.5, 0.75, 1.0, 1.25], "rgba16f read_write texture", "value \(value), expected [0.5, 0.75, 1.0, 1.25]")
            } else {
                compute["read_write_error"] = second.error ?? "unknown"
                emit(false, "rgba16f read_write texture", second.error ?? "unknown")
            }
        } catch {
            compute["read_write_error"] = "\(error)"
            emit(false, "rgba16f read_write texture", "pipeline: \(error)")
        }
    } catch {
        compute["error"] = "\(error)"
        emit(false, "rgba16f compute write", "pipeline: \(error)", required: true)
    }
    facts["rgba16f_compute"] = compute

    var archive: [String: Any] = [:]
    let archiveURL = artifacts.appendingPathComponent("probe-archive.metallib")
    do {
        let created = try device.makeBinaryArchive(descriptor: MTLBinaryArchiveDescriptor())
        try created.addRenderPipelineFunctions(descriptor: pipeline)
        let computeDescriptor = MTLComputePipelineDescriptor()
        computeDescriptor.computeFunction = library.makeFunction(name: "cs_fill")
        try created.addComputePipelineFunctions(descriptor: computeDescriptor)
        try created.serialize(to: archiveURL)
        let size = (try? FileManager.default.attributesOfItem(atPath: archiveURL.path)[.size] as? Int) ?? 0
        archive["serialized"] = true
        archive["bytes"] = size
        let reload = MTLBinaryArchiveDescriptor()
        reload.url = archiveURL
        do {
            let loaded = try device.makeBinaryArchive(descriptor: reload)
            pipeline.binaryArchives = [loaded]
            _ = try device.makeRenderPipelineState(descriptor: pipeline, options: [.failOnBinaryArchiveMiss])
            archive["reload_hit"] = true
        } catch {
            archive["reload_hit"] = false
            archive["reload_error"] = "\(error)"
        }
        emit(true, "MTLBinaryArchive serializeToURL", "\(size) bytes, reload without compiling: \(archive["reload_hit"] as? Bool ?? false)")
    } catch {
        archive["serialized"] = false
        archive["error"] = "\(error)"
        emit(false, "MTLBinaryArchive serializeToURL", "\(error)")
    }
    facts["binary_archive"] = archive
    return facts
}

func screenFacts() -> [[String: Any]] {
    NSScreen.screens.map { screen in
        let number = (screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value ?? 0
        let mode = CGDisplayCopyDisplayMode(number)
        let entry: [String: Any] = [
            "name": screen.localizedName,
            "display_id": number,
            "frame": [screen.frame.origin.x, screen.frame.origin.y, screen.frame.width, screen.frame.height],
            "visible_frame": [screen.visibleFrame.origin.x, screen.visibleFrame.origin.y, screen.visibleFrame.width, screen.visibleFrame.height],
            "backing_scale_factor": screen.backingScaleFactor,
            "maximum_frames_per_second": screen.maximumFramesPerSecond,
            "mode_refresh_rate": mode?.refreshRate ?? 0,
            "mode_pixels": [mode?.pixelWidth ?? 0, mode?.pixelHeight ?? 0],
            "color_space": screen.colorSpace?.localizedName ?? "unknown",
        ]
        return entry
    }
}

func samplePixel(_ url: URL) -> (rgb: [Int], size: [Int])? {
    guard let source = CGImageSourceCreateWithURL(url as CFURL, nil),
          let image = CGImageSourceCreateImageAtIndex(source, 0, nil),
          let space = CGColorSpace(name: CGColorSpace.sRGB),
          let context = CGContext(data: nil, width: image.width, height: image.height, bitsPerComponent: 8,
                                  bytesPerRow: image.width * 4, space: space,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else {
        return nil
    }
    context.draw(image, in: CGRect(x: 0, y: 0, width: image.width, height: image.height))
    guard let data = context.data else {
        return nil
    }
    let pixels = data.bindMemory(to: UInt8.self, capacity: image.width * image.height * 4)
    let offset = (Int(Double(image.height) * 0.6) * image.width + image.width / 2) * 4
    return ([Int(pixels[offset]), Int(pixels[offset + 1]), Int(pixels[offset + 2])], [image.width, image.height])
}

func inputSourceID(_ source: TISInputSource) -> String {
    guard let pointer = TISGetInputSourceProperty(source, kTISPropertyInputSourceID) else {
        return "unknown"
    }
    return Unmanaged<CFString>.fromOpaque(pointer).takeUnretainedValue() as String
}

func currentInputSourceID() -> String {
    guard let current = TISCopyCurrentKeyboardInputSource() else {
        return "none"
    }
    return inputSourceID(current.takeRetainedValue())
}

func kotoeriFacts(_ app: NSApplication) -> [String: Any] {
    var facts: [String: Any] = [:]
    guard let list = TISCreateInputSourceList(nil, true) else {
        facts["sources"] = []
        emit(false, "Kotoeri input source", "TISCreateInputSourceList returned nothing")
        return facts
    }
    let installed = list.takeRetainedValue() as NSArray as! [TISInputSource]
    facts["installed_count"] = installed.count
    let sources = installed.filter { inputSourceID($0).hasPrefix("com.apple.inputmethod.Kotoeri") }
    let ids = sources.map(inputSourceID)
    facts["sources"] = ids
    let original = TISCopyCurrentKeyboardInputSource()?.takeRetainedValue()
    facts["original"] = original.map(inputSourceID) ?? "none"
    guard let japanese = sources.first(where: { inputSourceID($0).hasSuffix("RomajiTyping.Japanese") })
        ?? sources.first(where: { inputSourceID($0).hasSuffix(".Japanese") }) else {
        emit(false, "Kotoeri input source", "no Japanese mode among \(ids) of \(installed.count) installed sources")
        return facts
    }
    let target = inputSourceID(japanese)
    facts["target"] = target
    facts["enable_status"] = Dictionary(uniqueKeysWithValues: sources.map { (inputSourceID($0), Int(TISEnableInputSource($0))) })
    let selectStatus = TISSelectInputSource(japanese)
    facts["select_status"] = Int(selectStatus)
    pump(app, 2, until: { currentInputSourceID() == target })
    let selected = currentInputSourceID()
    facts["selected"] = selected
    facts["selected_matches"] = selected == target
    emit(selected == target, "Kotoeri input source", "enable \(facts["enable_status"] ?? ""), select status \(selectStatus), current \(selected)")
    if let original {
        facts["restore_status"] = Int(TISSelectInputSource(original))
    }
    return facts
}

func accessibilityFacts(_ app: NSApplication) -> [String: Any] {
    var facts: [String: Any] = ["trusted": AXIsProcessTrusted()]
    let child = Process()
    child.executableURL = Bundle.main.executableURL ?? URL(fileURLWithPath: CommandLine.arguments[0])
    child.arguments = ["--ax-child"]
    do {
        try child.run()
    } catch {
        facts["child_error"] = "\(error)"
        emit(false, "AXUIElement of a child app", "launch failed: \(error)")
        return facts
    }
    let element = AXUIElementCreateApplication(child.processIdentifier)
    AXUIElementSetMessagingTimeout(element, 2)
    var titles: [String] = []
    var lastError = AXError.success
    let deadline = Date().addingTimeInterval(10)
    while Date() < deadline && titles.isEmpty {
        pump(app, 0.5)
        var value: CFTypeRef?
        lastError = AXUIElementCopyAttributeValue(element, kAXWindowsAttribute as CFString, &value)
        let windows = (value as? [AXUIElement]) ?? []
        titles = windows.compactMap { window in
            var title: CFTypeRef?
            AXUIElementCopyAttributeValue(window, kAXTitleAttribute as CFString, &title)
            return title as? String
        }
    }
    child.terminate()
    child.waitUntilExit()
    facts["child_window_titles"] = titles
    facts["child_ax_error"] = Int(lastError.rawValue)
    let reached = titles.contains("aqua-probe-child")
    facts["child_reached"] = reached
    emit(reached, "AXUIElement of a child app", "AXIsProcessTrusted \(AXIsProcessTrusted()), AXError \(lastError.rawValue), window titles \(titles)")
    return facts
}

func memoryPressureFacts(_ app: NSApplication) -> [String: Any] {
    var facts: [String: Any] = [:]
    let queue = DispatchQueue(label: "aqua.probe.pressure")
    let source = DispatchSource.makeMemoryPressureSource(eventMask: [.normal, .warning, .critical], queue: queue)
    source.setEventHandler {
        let data = source.data
        let levels = [(DispatchSource.MemoryPressureEvent.normal, "normal"), (.warning, "warning"), (.critical, "critical")]
        counters.note(levels.filter { data.contains($0.0) }.map { $0.1 }.joined(separator: "+"))
    }
    source.resume()
    let simulate = run(app, "/usr/bin/sudo", ["-n", "/usr/bin/memory_pressure", "-S", "-l", "warn"], timeout: 15)
    facts["simulate_status"] = simulate.status.map { Int($0) as Any } ?? NSNull()
    facts["simulate_output"] = String(simulate.output.prefix(400))
    pump(app, 10, until: { !counters.recorded().isEmpty })
    let reset = run(app, "/usr/bin/sudo", ["-n", "/usr/bin/memory_pressure", "-S", "-l", "normal"], timeout: 15)
    facts["reset_status"] = reset.status.map { Int($0) as Any } ?? NSNull()
    pump(app, 2)
    source.cancel()
    let events = counters.recorded()
    facts["events"] = events
    let fired = events.contains { $0.contains("warning") }
    facts["warning_fired"] = fired
    emit(fired, "memory pressure dispatch source", "sudo memory_pressure -S -l warn exit \(simulate.status.map { String($0) } ?? "none"), events \(events)")
    return facts
}

func windowFacts(_ app: NSApplication, _ artifacts: URL) -> [String: Any] {
    var facts: [String: Any] = [:]
    let window = NSWindow(contentRect: NSRect(x: 240, y: 240, width: 320, height: 240), styleMask: [.titled, .closable],
                          backing: .buffered, defer: false)
    window.title = "aqua-probe"
    window.isReleasedWhenClosed = false
    window.backgroundColor = NSColor(srgbRed: 0.2, green: 0.6, blue: 0.9, alpha: 1)
    let view = KeyView(frame: NSRect(x: 0, y: 0, width: 320, height: 240))
    view.wantsLayer = true
    view.layer?.backgroundColor = CGColor(srgbRed: 0.2, green: 0.6, blue: 0.9, alpha: 1)
    window.contentView = view
    window.makeKeyAndOrderFront(nil)
    window.makeFirstResponder(view)
    app.activate()
    pump(app, 3, until: { app.isActive && window.isKeyWindow })
    facts["active_after_activate"] = app.isActive
    facts["key_after_activate"] = window.isKeyWindow
    if !app.isActive || !window.isKeyWindow {
        app.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
        pump(app, 3, until: { app.isActive && window.isKeyWindow })
    }
    facts["active"] = app.isActive
    facts["key"] = window.isKeyWindow
    facts["main"] = window.isMainWindow
    facts["visible"] = window.isVisible
    facts["occlusion_visible"] = window.occlusionState.contains(.visible)
    facts["window_number"] = window.windowNumber
    facts["window_backing_scale"] = window.backingScaleFactor
    let front = NSWorkspace.shared.frontmostApplication
    facts["frontmost_app"] = front?.localizedName ?? "none"
    facts["frontmost_is_probe"] = front?.processIdentifier == getpid()
    let screens = screenFacts()
    facts["screens"] = screens
    emit(window.isVisible, "NSWindow ordered front", "visible \(window.isVisible), occlusion visible \(window.occlusionState.contains(.visible)), window \(window.windowNumber)")
    emit(app.isActive && window.isKeyWindow, "key window and active app", "activate() active \(facts["active_after_activate"] ?? false) key \(facts["key_after_activate"] ?? false); after activate(ignoringOtherApps:) active \(app.isActive) key \(window.isKeyWindow); frontmost \(facts["frontmost_app"] ?? "")")
    emit(!screens.isEmpty, "NSScreen", screens.map { "\($0["name"] ?? "") frame \($0["frame"] ?? "") scale \($0["backing_scale_factor"] ?? "") max fps \($0["maximum_frames_per_second"] ?? "") mode \($0["mode_pixels"] ?? "")@\($0["mode_refresh_rate"] ?? "")" }.joined(separator: "; "))

    let trusted = AXIsProcessTrusted()
    let capture = CGPreflightScreenCaptureAccess()
    let post = CGPreflightPostEventAccess()
    facts["tcc"] = ["ax_trusted": trusted, "screen_capture": capture, "post_event": post, "listen_event": CGPreflightListenEventAccess()]
    emit(trusted, "TCC accessibility (AXIsProcessTrusted)", "\(trusted)")
    emit(capture, "TCC screen capture (CGPreflightScreenCaptureAccess)", "\(capture)")
    emit(post, "TCC post event (CGPreflightPostEventAccess)", "\(post)")

    pump(app, 1)
    let shot = artifacts.appendingPathComponent("probe-window.png")
    let captured = run(app, "/usr/sbin/screencapture", ["-x", "-o", "-l\(window.windowNumber)", shot.path], timeout: 20)
    var capturedFacts: [String: Any] = ["status": captured.status.map { Int($0) as Any } ?? NSNull(), "output": String(captured.output.prefix(400)), "expected": [51, 153, 230]]
    if captured.status == 0, let sample = samplePixel(shot) {
        let matches = zip(sample.rgb, [51, 153, 230]).allSatisfy { abs($0 - $1) <= 8 }
        capturedFacts["pixel"] = sample.rgb
        capturedFacts["size"] = sample.size
        capturedFacts["matches"] = matches
        emit(matches, "screencapture -l of the window", "pixel \(sample.rgb) expected [51, 153, 230], image \(sample.size)")
    } else {
        capturedFacts["matches"] = false
        emit(false, "screencapture -l of the window", "exit \(captured.status.map { String($0) } ?? "none"): \(captured.output.prefix(200))")
    }
    facts["screencapture"] = capturedFacts
    _ = run(app, "/usr/sbin/screencapture", ["-x", artifacts.appendingPathComponent("probe-screen.png").path], timeout: 20)

    let monitor = NSEvent.addLocalMonitorForEvents(matching: [.keyDown]) { event in
        counters.bump("app_key_down")
        return event
    }
    var posting: [String: Any] = [:]
    let eventSource = CGEventSource(stateID: .hidSystemState)
    for (name, deliver) in [("hid_tap", { (event: CGEvent) in event.post(tap: .cghidEventTap) }),
                            ("session_tap", { (event: CGEvent) in event.post(tap: .cgSessionEventTap) }),
                            ("post_to_pid", { (event: CGEvent) in event.postToPid(getpid()) })] {
        counters.reset()
        window.makeKeyAndOrderFront(nil)
        window.makeFirstResponder(view)
        pump(app, 0.3)
        if let down = CGEvent(keyboardEventSource: eventSource, virtualKey: 0, keyDown: true),
           let up = CGEvent(keyboardEventSource: eventSource, virtualKey: 0, keyDown: false) {
            deliver(down)
            deliver(up)
        }
        pump(app, 2, until: { counters.count("view_key_down") > 0 })
        posting[name] = ["app_key_down": counters.count("app_key_down"), "view_key_down": counters.count("view_key_down")]
        emit(counters.count("view_key_down") > 0, "CGEventPost key (\(name))", "app saw \(counters.count("app_key_down")), view saw \(counters.count("view_key_down"))")
    }
    if let monitor {
        NSEvent.removeMonitor(monitor)
    }
    facts["cg_event_post"] = posting
    window.orderOut(nil)
    return facts
}

final class Measured: @unchecked Sendable {
    var samples: [String: [Double]] = [:]
}

func timedWaitFacts(_ label: String, _ qos: QualityOfService) -> [String: Any] {
    let requested = 0.004
    let rounds = 25
    let done = DispatchSemaphore(value: 0)
    let measured = Measured()
    let worker = Thread {
        let condition = NSCondition()
        let waits: [(String, () -> Void)] = [
            ("thread_sleep", { Thread.sleep(forTimeInterval: requested) }),
            ("condition_wait", {
                condition.lock()
                _ = condition.wait(until: Date().addingTimeInterval(requested))
                condition.unlock()
            }),
            ("semaphore_wait", { _ = DispatchSemaphore(value: 0).wait(timeout: .now() + requested) }),
        ]
        for (name, wait) in waits {
            measured.samples[name] = (0..<rounds).map { _ in
                let start = DispatchTime.now().uptimeNanoseconds
                wait()
                return Double(DispatchTime.now().uptimeNanoseconds - start) / 1_000_000
            }
        }
        done.signal()
    }
    worker.qualityOfService = qos
    worker.start()
    done.wait()
    var facts: [String: Any] = ["requested_ms": requested * 1000, "rounds": rounds]
    var summary: [String] = []
    var precise = true
    for name in measured.samples.keys.sorted() {
        let samples = measured.samples[name] ?? []
        let mean = samples.reduce(0, +) / Double(max(samples.count, 1))
        let worst = samples.max() ?? 0
        facts[name] = ["mean_ms": mean, "max_ms": worst]
        summary.append("\(name) mean \(String(format: "%.2f", mean)) ms max \(String(format: "%.2f", worst)) ms")
        precise = precise && mean < requested * 1000 * 1.5
    }
    facts["precise"] = precise
    emit(precise, "4 ms timed waits (\(label))", summary.joined(separator: ", "))
    return facts
}

func childMain() -> Never {
    let app = launch()
    let window = NSWindow(contentRect: NSRect(x: 600, y: 240, width: 200, height: 120), styleMask: [.titled],
                          backing: .buffered, defer: false)
    window.title = "aqua-probe-child"
    window.isReleasedWhenClosed = false
    window.orderFront(nil)
    pump(app, 30)
    exit(0)
}

let arguments = CommandLine.arguments
if arguments.count > 1 && arguments[1] == "--ax-child" {
    childMain()
}
guard arguments.count >= 3 else {
    FileHandle.standardError.write("usage: macos-probe <probe.json> <artifacts dir> [key=value ...]\n".data(using: .utf8)!)
    exit(2)
}
let output = URL(fileURLWithPath: arguments[1])
let artifacts = URL(fileURLWithPath: arguments[2], isDirectory: true)
try? FileManager.default.createDirectory(at: artifacts, withIntermediateDirectories: true)
var probe: [String: Any] = [:]
probe["toolchain"] = Dictionary(arguments.dropFirst(3).compactMap { pair -> (String, String)? in
    guard let split = pair.firstIndex(of: "=") else { return nil }
    return (String(pair[..<split]), String(pair[pair.index(after: split)...]))
}, uniquingKeysWith: { _, last in last })
probe["os"] = ProcessInfo.processInfo.operatingSystemVersionString
probe["physical_memory"] = ProcessInfo.processInfo.physicalMemory
probe["processors"] = ProcessInfo.processInfo.activeProcessorCount
probe["metal"] = metalFacts(artifacts)
var waits: [String: Any] = [
    "default_qos_before_app": timedWaitFacts("default QoS, before NSApplication", .default),
    "user_interactive_before_app": timedWaitFacts("user-interactive QoS, before NSApplication", .userInteractive),
]
let app = launch()
probe["window"] = windowFacts(app, artifacts)
waits["default_qos_active_app"] = timedWaitFacts("default QoS, app active", .default)
waits["user_interactive_active_app"] = timedWaitFacts("user-interactive QoS, app active", .userInteractive)
probe["timed_waits"] = waits
probe["kotoeri"] = kotoeriFacts(app)
probe["accessibility"] = accessibilityFacts(app)
probe["memory_pressure"] = memoryPressureFacts(app)
do {
    let data = try JSONSerialization.data(withJSONObject: probe, options: [.prettyPrinted, .sortedKeys])
    try data.write(to: output)
    emit(true, "probe.json", output.path, required: true)
} catch {
    emit(false, "probe.json", "\(error)", required: true)
}
exit(0)
