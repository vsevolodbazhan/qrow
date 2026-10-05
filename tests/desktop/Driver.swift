import AppKit
import ApplicationServices
import Foundation

// This driver uses macOS input and accessibility APIs against the packaged application.
// It does not call Qrow internals or replace its connector.
struct Failure: Error, CustomStringConvertible {
    let description: String
    init(_ description: String) { self.description = description }
}
let env = ProcessInfo.processInfo.environment
let artifacts = env["QROW_E2E_ARTIFACTS"] ?? "/tmp"
let clock = ContinuousClock()
var inputPID: pid_t = 0

func require(_ condition: Bool, _ message: String) throws {
    if !condition { throw Failure(message) }
}
func attribute(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
    var value: CFTypeRef?
    return AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success ? value : nil
}
func strings(_ element: AXUIElement) -> [String] {
    [kAXTitleAttribute, kAXDescriptionAttribute, kAXValueAttribute, "AXHelp", "AXIdentifier"]
        .compactMap { attribute(element, $0) as? String }
}
func containsText(_ fragment: String, in root: AXUIElement) -> Bool {
    descendants(root).contains { strings($0).contains { $0.contains(fragment) } }
}
func descendants(_ root: AXUIElement) -> [AXUIElement] {
    var queue = [root]
    var result: [AXUIElement] = []
    var seen = Set<AXUIElement>()
    while !queue.isEmpty && result.count < 10000 {
        let element = queue.removeFirst()
        if !seen.insert(element).inserted { continue }
        result.append(element)
        queue += (attribute(element, kAXChildrenAttribute) as? [AXUIElement]) ?? []
    }
    return result
}
func key(_ code: CGKeyCode, flags: CGEventFlags = []) {
    for down in [true, false] {
        let event = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: down)!
        event.flags = flags
        event.postToPid(inputPID)
    }
}
func elementBounds(_ element: AXUIElement) throws -> (CGPoint, CGSize) {
    let deadline = clock.now.advanced(by: .seconds(5))
    var position: CFTypeRef?
    var size: CFTypeRef?
    repeat {
        position = attribute(element, kAXPositionAttribute)
        size = attribute(element, kAXSizeAttribute)
        if position != nil && size != nil { break }
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
    } while clock.now < deadline
    guard let position, let size else { throw Failure("Element has no bounds: \(strings(element))") }
    var point = CGPoint.zero
    var extent = CGSize.zero
    try require(CFGetTypeID(position) == AXValueGetTypeID() && CFGetTypeID(size) == AXValueGetTypeID(), "Invalid element bounds")
    AXValueGetValue(unsafeBitCast(position, to: AXValue.self), .cgPoint, &point)
    AXValueGetValue(unsafeBitCast(size, to: AXValue.self), .cgSize, &extent)
    return (point, extent)
}
/// Every run uses the Qrow window size of a hosted macOS runner, whose main
/// display is 1024 by 768 points. Local runs then show the same layout, menu
/// placement, and pane widths as CI runs.
let testWindowSize = CGSize(width: 992, height: 652)
/// Sets the Qrow window to `testWindowSize` near the top of the main display.
func setTestWindowFrame(_ app: AXUIElement) throws {
    _ = NSApplication.shared
    guard let window = (attribute(app, kAXWindowsAttribute) as? [AXUIElement])?.first,
          let screen = NSScreen.screens.first else {
        throw Failure("Qrow window or main display is unavailable")
    }
    let visible = screen.visibleFrame
    try require(
        visible.width - 32 >= testWindowSize.width && visible.height - 32 >= testWindowSize.height,
        "Main display is too small for the \(Int(testWindowSize.width)) by \(Int(testWindowSize.height)) point test window"
    )
    let top = screen.frame.maxY - visible.maxY
    let safeFrame = CGRect(x: visible.minX, y: top, width: visible.width, height: visible.height)

    var size = testWindowSize
    guard let sizeValue = AXValueCreate(.cgSize, &size) else {
        throw Failure("Could not create the Qrow window size")
    }
    try require(
        AXUIElementSetAttributeValue(window, kAXSizeAttribute as CFString, sizeValue) == .success,
        "Could not resize the Qrow window for the test"
    )
    var position = CGPoint(x: visible.minX + (visible.width - size.width) / 2, y: top + 16)
    guard let positionValue = AXValueCreate(.cgPoint, &position) else {
        throw Failure("Could not create the Qrow window position")
    }
    try require(
        AXUIElementSetAttributeValue(window, kAXPositionAttribute as CFString, positionValue) == .success,
        "Could not move the Qrow window onto the main display"
    )
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.5))

    let (actualPosition, actualSize) = try elementBounds(window)
    try require(
        abs(actualSize.width - size.width) <= 1 && abs(actualSize.height - size.height) <= 1,
        "Qrow window is \(actualSize), not the \(size) test window"
    )
    try require(
        safeFrame.insetBy(dx: -1, dy: -1).contains(CGRect(origin: actualPosition, size: actualSize)),
        "Qrow window is outside the main display after repositioning"
    )
    print("Set the Qrow test window: \(actualPosition) \(actualSize)")
}
/// Returns the point to click for an element. Query tabs scroll out of the tab
/// strip at its start, and under the fixed New Tab control at its end. The
/// middle of a partly hidden tab can then be outside the strip or over that
/// control, so for a tab the point is the middle of its visible part.
func clickTarget(_ element: AXUIElement, _ point: CGPoint, _ extent: CGSize) -> CGPoint {
    var target = CGPoint(x: point.x + extent.width / 2, y: point.y + extent.height / 2)
    guard attribute(element, kAXRoleAttribute) as? String == kAXRadioButtonRole,
          let window = attribute(element, kAXWindowAttribute) else { return target }
    var left = point.x
    var right = point.x + extent.width
    // The strip clips its tabs, so a tab shows only inside its parent.
    if let parent = attribute(element, kAXParentAttribute),
       attribute(unsafeBitCast(parent, to: AXUIElement.self), kAXPositionAttribute) != nil,
       let (stripPoint, stripExtent) = try? elementBounds(unsafeBitCast(parent, to: AXUIElement.self)),
       stripExtent.width > 0 {
        left = max(left, stripPoint.x)
        right = min(right, stripPoint.x + stripExtent.width)
    }
    for control in descendants(unsafeBitCast(window, to: AXUIElement.self)) {
        guard attribute(control, kAXRoleAttribute) as? String == kAXButtonRole,
              strings(control).first == "New Tab",
              let (controlPoint, controlExtent) = try? elementBounds(control),
              abs(controlPoint.y + controlExtent.height / 2 - target.y) < extent.height / 2 else { continue }
        right = min(right, controlPoint.x)
    }
    if left < right { target.x = (left + right) / 2 }
    return target
}
func click(_ element: AXUIElement) throws {
    // Dialog accessibility nodes appear before their opening animation settles.
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.3))
    let (point, extent) = try elementBounds(element)
    print("Click \(strings(element)): \(point) \(extent)")
    let clickPoint = clickTarget(element, point, extent)
    for eventType in [CGEventType.leftMouseDown, .leftMouseUp] {
        let event = CGEvent(mouseEventSource: nil, mouseType: eventType, mouseCursorPosition: clickPoint, mouseButton: .left)!
        event.setIntegerValueField(.mouseEventClickState, value: 1)
        event.flags = []
        event.post(tap: .cghidEventTap)
    }
}
func clickPoint(_ point: CGPoint) {
    for eventType in [CGEventType.leftMouseDown, .leftMouseUp] {
        let event = CGEvent(mouseEventSource: nil, mouseType: eventType, mouseCursorPosition: point, mouseButton: .left)!
        event.post(tap: .cghidEventTap)
    }
}
func rightClick(_ element: AXUIElement) throws {
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.3))
    let (point, extent) = try elementBounds(element)
    let clickPoint = clickTarget(element, point, extent)
    print("Right click \(strings(element)): \(point) \(extent)")
    for eventType in [CGEventType.rightMouseDown, .rightMouseUp] {
        let event = CGEvent(mouseEventSource: nil, mouseType: eventType, mouseCursorPosition: clickPoint, mouseButton: .right)!
        event.setIntegerValueField(.mouseEventClickState, value: 1)
        event.flags = []
        event.post(tap: .cghidEventTap)
    }
}
func scrollDown(_ element: AXUIElement) throws {
    // GPUI exposes the form fields through Accessibility, but the current
    // scroll container does not expose AXScrollToVisible on macOS. Send the
    // wheel event over a visible form control as a compatible fallback.
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.3))
    let (point, extent) = try elementBounds(element)
    var scrollPoint = point
    scrollPoint.x += extent.width / 2
    scrollPoint.y += extent.height / 2
    let move = CGEvent(mouseEventSource: nil, mouseType: .mouseMoved, mouseCursorPosition: scrollPoint, mouseButton: .left)!
    move.post(tap: .cghidEventTap)
    let scroll = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: -1200, wheel2: 0, wheel3: 0)!
    scroll.location = scrollPoint
    scroll.post(tap: .cghidEventTap)
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.3))
}

func scrollUpAbove(_ element: AXUIElement, by distance: Int32 = 1200) throws {
    let (point, extent) = try elementBounds(element)
    let location = CGPoint(x: point.x + extent.width / 2, y: point.y - 120)
    let move = CGEvent(mouseEventSource: nil, mouseType: .mouseMoved, mouseCursorPosition: location, mouseButton: .left)!
    move.post(tap: .cghidEventTap)
    let scroll = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: distance, wheel2: 0, wheel3: 0)!
    scroll.location = location
    scroll.post(tap: .cghidEventTap)
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.3))
}
func command(_ args: [String]) throws -> String {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
    process.arguments = args
    let output = Pipe()
    process.standardOutput = output
    try process.run()
    let data = output.fileHandleForReading.readDataToEndOfFile()
    process.waitUntilExit()
    try require(process.terminationStatus == 0, "Command failed: \(args.first ?? "")")
    return String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
}

final class Driver {
    let name: String
    init(name: String = "qrow") { self.name = name }
    let process = Process()
    var app: AXUIElement!
    var log: FileHandle!
    var sampleTimer: Timer?
    var samples = [String]()
    func elements() -> [AXUIElement] {
        // macOS menus can contain the user's recent files. Inspect only this app's windows.
        ((attribute(app, kAXWindowsAttribute) as? [AXUIElement]) ?? []).flatMap(descendants)
    }
    func find(_ label: String, role: String? = nil) -> AXUIElement? {
        elements().first {
            (role == nil || attribute($0, kAXRoleAttribute) as? String == role) && strings($0).contains { $0.contains(label) }
        }
    }
    func findExact(_ label: String, role: String? = nil) -> AXUIElement? {
        elements().first {
            (role == nil || attribute($0, kAXRoleAttribute) as? String == role) && strings($0).contains(where: { $0 == label })
        }
    }
    func wait(_ label: String, timeout: Double = 150, role: String? = nil) throws -> AXUIElement {
        let deadline = clock.now.advanced(by: .seconds(timeout))
        repeat {
            if let element = find(label, role: role) { return element }
            try require(process.isRunning, "Qrow exited while waiting for \(label)")
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
        } while clock.now < deadline
        throw Failure("Timed out waiting for \(label)")
    }
    func waitAny(_ labels: [String], timeout: Double = 150) throws -> AXUIElement {
        let deadline = clock.now.advanced(by: .seconds(timeout))
        repeat {
            for label in labels {
                if let element = find(label) { return element }
            }
            try require(process.isRunning, "Qrow exited while waiting for \(labels.joined(separator: ", "))")
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
        } while clock.now < deadline
        throw Failure("Timed out waiting for \(labels.joined(separator: ", "))")
    }
    func waitExact(_ label: String, timeout: Double = 150, role: String? = nil) throws -> AXUIElement {
        let deadline = clock.now.advanced(by: .seconds(timeout))
        repeat {
            if let element = findExact(label, role: role) {
                return element
            }
            try require(process.isRunning, "Qrow exited while waiting for \(label)")
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
        } while clock.now < deadline
        throw Failure("Timed out waiting for \(label)")
    }
    func waitGone(_ label: String, timeout: Double = 10, role: String? = nil) throws {
        let deadline = clock.now.advanced(by: .seconds(timeout))
        while find(label, role: role) != nil {
            try require(clock.now < deadline, "Old UI state remained visible: \(label)")
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.05))
        }
    }
    func press(_ label: String, pointer: Bool = false) throws {
        let deadline = clock.now.advanced(by: .seconds(150))
        repeat {
            let control = find(label, role: kAXButtonRole) ?? find(label, role: kAXCheckBoxRole)
            if let control, attribute(control, kAXEnabledAttribute) as? Bool != false {
                // Prefer the control's native accessibility action. GPUI Kit
                // alert buttons can be present in the accessibility tree
                // before their hit-test surface is ready for a pointer click.
                if pointer || AXUIElementPerformAction(control, kAXPressAction as CFString) != .success {
                    try click(control)
                }
                return
            }
            try require(process.isRunning, "Qrow exited while waiting for button: \(label)")
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
        } while clock.now < deadline
        throw Failure("Button never became enabled: \(label)")
    }
    func activate(_ element: AXUIElement) throws {
        if AXUIElementPerformAction(element, kAXPressAction as CFString) != .success {
            try click(element)
        }
    }
    func pressMenuItem(_ label: String) throws {
        // Menu items are transient. Use their accessibility action instead of
        // a screen coordinate that can be stale on scaled or multi-display
        // configurations.
        try activate(try wait(label))
    }
    func scrollIntoView(_ element: AXUIElement) {
        // Use the standard action when the target's scroll container publishes
        // it. The connection form also has a wheel fallback at its lifecycle
        // entry points because its current container does not publish it.
        _ = AXUIElementPerformAction(element, "AXScrollToVisible" as CFString)
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.3))
    }
    func accessibleInput(_ label: String) -> AXUIElement? {
        elements().first {
            [kAXTextFieldRole, kAXTextAreaRole].contains(attribute($0, kAXRoleAttribute) as? String ?? "") && strings($0).contains(label)
        }
    }
    func waitInput(_ label: String, timeout: Double = 10) throws -> AXUIElement {
        let deadline = clock.now.advanced(by: .seconds(timeout))
        repeat {
            if let input = accessibleInput(label) { return input }
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.05))
        } while clock.now < deadline
        throw Failure("Missing accessible input: \(label)")
    }
    func waitInputValue(_ label: String, _ expected: String, timeout: Double = 10) throws {
        let deadline = clock.now.advanced(by: .seconds(timeout))
        repeat {
            if let input = accessibleInput(label),
               attribute(input, kAXValueAttribute) as? String == expected { return }
            try require(process.isRunning, "Qrow exited while waiting for \(label)")
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.05))
        } while clock.now < deadline
        throw Failure("\(label) did not become \(expected)")
    }
    func fill(_ label: String, _ value: String) throws {
        // GPUI exposes inputs as text fields or text areas depending on the control.
        var element = try waitInput(label)
        scrollIntoView(element)
        // Re-read the node after scrolling because its bounds can change with
        // the scroll offset.
        element = try waitInput(label)
        try click(element)
        // The GPUI Kit input is exposed as a settable accessibility element.
        // Explicitly focus it as well as clicking its bounds so keyboard input
        // remains deterministic across display scaling configurations.
        try require(
            AXUIElementSetAttributeValue(element, kAXFocusedAttribute as CFString, kCFBooleanTrue) == .success,
            "Could not focus input: \(label)"
        )
        // Let the native click establish editor focus before sending keyboard shortcuts.
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.2))
        key(0, flags: .maskCommand)
        // GPUI processes the selection action asynchronously. Let it settle
        // before replacing the selection through the pasteboard.
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
        let clipboard = NSPasteboard.general
        let saved = (clipboard.pasteboardItems ?? []).map { item in
            item.types.compactMap { type -> (NSPasteboard.PasteboardType, Data)? in
                item.data(forType: type).map { (type, $0) }
            }
        }
        defer {
            clipboard.clearContents()
            let restored = saved.map { data -> NSPasteboardItem in
                let item = NSPasteboardItem()
                for (type, value) in data { item.setData(value, forType: type) }
                return item
            }
            clipboard.writeObjects(restored)
        }
        clipboard.clearContents()
        clipboard.setString(value, forType: .string)
        key(9, flags: .maskCommand)
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
        // The current GPUI Kit input exposes AccessKit's SetValue action, but
        // some macOS environments do not deliver synthetic Cmd+V events to
        // that input. Keep the real click and keyboard path, then use the
        // published accessibility action when the value did not arrive.
        let keyboardDeadline = clock.now.advanced(by: .seconds(1))
        while label == "Password" || accessibleInput(label).flatMap({ attribute($0, kAXValueAttribute) as? String }) != value {
            if clock.now >= keyboardDeadline { break }
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.05))
        }
        if label == "Password" || accessibleInput(label).flatMap({ attribute($0, kAXValueAttribute) as? String }) != value {
            let input = try waitInput(label)
            try require(
                AXUIElementSetAttributeValue(input, kAXValueAttribute as CFString, value as CFString) == .success,
                "Input accessibility action failed: \(label)"
            )
            if label != "Password" {
                let deadline = clock.now.advanced(by: .seconds(5))
                while accessibleInput(label).flatMap({ attribute($0, kAXValueAttribute) as? String }) != value {
                    try require(clock.now < deadline, "Input did not accept text: \(label)")
                    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.05))
                }
            }
        }
    }
    func query(_ sql: String) throws {
        try fill("SQL Editor", sql)
        try press("Run")
    }
    func snapshot(_ name: String) throws {
        let text = elements().map { "\(attribute($0, kAXRoleAttribute) ?? "?" as CFString) \(strings($0))" }.joined(separator: "\n")
        try text.write(toFile: "\(artifacts)/\(name)-accessibility.txt", atomically: true, encoding: .utf8)
        let (number, _) = try qrowWindow()
        _ = try command(["screencapture", "-x", "-l", "\(number)", "\(artifacts)/\(name).png"])
    }
    /// The number of the Qrow window and its frame in screen points.
    func qrowWindow() throws -> (UInt32, CGRect) {
        let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
        guard let window = windows.first(where: {
            $0[kCGWindowOwnerPID as String] as? Int32 == process.processIdentifier && $0[kCGWindowLayer as String] as? Int == 0
        }), let number = window[kCGWindowNumber as String] as? UInt32,
              let bounds = window[kCGWindowBounds as String] as? NSDictionary,
              let frame = CGRect(dictionaryRepresentation: bounds) else { throw Failure("No Qrow window to capture") }
        return (number, frame)
    }
    /// Saves the pixels of `rect` ("x,y,width,height" in screen points) as a
    /// PNG at `path`. The pixels come from the Qrow window alone, so other
    /// windows, their shadows, and notifications do not change them. The
    /// capture repeats until two captures in a row match, so a frame of an
    /// animation or a scroll does not count.
    func captureRegion(_ rect: String, to path: String) throws {
        let parts = rect.split(separator: ",").compactMap { Double($0) }
        try require(parts.count == 4, "Invalid capture region: \(rect)")
        let region = CGRect(x: parts[0], y: parts[1], width: parts[2], height: parts[3])
        let whole = "\(artifacts)/window-capture.png"
        defer { try? FileManager.default.removeItem(atPath: whole) }
        var previous: Data?
        for _ in 0..<15 {
            let (number, frame) = try qrowWindow()
            _ = try command(["screencapture", "-x", "-o", "-l", "\(number)", whole])
            guard let data = FileManager.default.contents(atPath: whole),
                  let bitmap = NSBitmapImageRep(data: data), let image = bitmap.cgImage else {
                throw Failure("Could not read the Qrow window capture")
            }
            let scale = CGFloat(bitmap.pixelsWide) / frame.width
            let crop = CGRect(x: (region.minX - frame.minX) * scale, y: (region.minY - frame.minY) * scale,
                              width: region.width * scale, height: region.height * scale).integral
            guard let cropped = image.cropping(to: crop),
                  let png = NSBitmapImageRep(cgImage: cropped).representation(using: .png, properties: [:]) else {
                throw Failure("The capture region \(rect) is outside the Qrow window")
            }
            if png == previous {
                try png.write(to: URL(fileURLWithPath: path))
                return
            }
            previous = png
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.2))
        }
        throw Failure("The Qrow window did not stop changing in \(rect)")
    }
    func start() throws {
        let bundle = env["QROW_E2E_BUNDLE"]!
        let logURL = URL(fileURLWithPath: "\(artifacts)/\(name).log")
        FileManager.default.createFile(atPath: logURL.path, contents: nil)
        log = try FileHandle(forWritingTo: logURL)
        process.executableURL = URL(fileURLWithPath: "\(bundle)/Contents/MacOS/qrow")
        let demo = CommandLine.arguments.contains("--editor-highlight-only")
            || CommandLine.arguments.contains("--results-text-only")
        if demo {
            process.arguments = ["--demo"]
        }
        process.environment = env
        process.standardOutput = log
        process.standardError = log
        let started = clock.now
        try process.run()
        inputPID = process.processIdentifier
        app = AXUIElementCreateApplication(process.processIdentifier)
        NSRunningApplication(processIdentifier: process.processIdentifier)?.activate(options: [])
        _ = try wait(demo ? "SQL Editor" : "New Connection", timeout: 20)
        samples.append("launch_to_accessible_new_connection_seconds=\(started.duration(to: clock.now))")
        try setTestWindowFrame(app)
        sampleTimer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
            guard let self else { return }
            if let sample = try? command(["ps", "-o", "rss=,%cpu=", "-p", "\(self.process.processIdentifier)"]) {
                self.samples.append("\(Date().timeIntervalSince1970) \(sample)")
            }
        }
    }
    func requestWindowClose() throws {
        guard let window = (attribute(app, kAXWindowsAttribute) as? [AXUIElement])?.first,
              let close = attribute(window, kAXCloseButtonAttribute) else {
            throw Failure("Window close button is unavailable")
        }
        try click(unsafeBitCast(close, to: AXUIElement.self))
    }
    func testEditorHighlight() throws {
        let editor = try waitInput("SQL Editor")
        let (origin, extent) = try elementBounds(editor)
        clickPoint(CGPoint(x: origin.x + 100, y: origin.y + 20))
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.3))

        // Capture only the editor's right edge. The reference column is inside
        // the text area; the edge column is inside the right padding.
        let captureWidth = 40
        let captureHeight = 90
        let path = "\(artifacts)/editor-highlight.png"
        let rect = "\(Int(origin.x + extent.width) - captureWidth),\(Int(origin.y)),\(captureWidth),\(captureHeight)"
        try captureRegion(rect, to: path)
        guard let data = FileManager.default.contents(atPath: path),
              let bitmap = NSBitmapImageRep(data: data) else {
            throw Failure("Could not read editor highlight capture")
        }
        let scale = Double(bitmap.pixelsWide) / Double(captureWidth)
        let referenceX = Int(15 * scale)
        let edgeX = Int(38 * scale)
        func color(_ x: Int, _ y: Int) -> NSColor? {
            bitmap.colorAt(x: x, y: y)?.usingColorSpace(.deviceRGB)
        }
        func distance(_ a: NSColor, _ b: NSColor) -> CGFloat {
            max(abs(a.redComponent - b.redComponent),
                abs(a.greenComponent - b.greenComponent),
                abs(a.blueComponent - b.blueComponent))
        }
        guard let background = color(referenceX, bitmap.pixelsHigh - 5) else {
            throw Failure("Could not sample editor background")
        }
        let highlightedRows = (5..<(bitmap.pixelsHigh - 5)).filter { y in
            color(referenceX, y).map { distance($0, background) > 0.015 } ?? false
        }
        try require(highlightedRows.count >= Int(8 * scale), "Active line highlight was not visible")
        let middle = highlightedRows[highlightedRows.count / 2]
        guard let reference = color(referenceX, middle), let edge = color(edgeX, middle) else {
            throw Failure("Could not sample active line highlight")
        }
        try require(distance(reference, edge) < 0.015, "Active line highlight stops before the editor's right edge")
        print("PASS: The active line highlight reaches the right edge of the editor")
    }
    func testResultsText() throws {
        let descender = try wait("Singapore Airlines", timeout: 20, role: kAXCellRole)
        let (descenderOrigin, descenderSize) = try elementBounds(descender)
        // Routes such as "HEL → ARN" have no descenders, so their ink ends at the baseline of the same row.
        guard let baseline = elements().first(where: {
            attribute($0, kAXRoleAttribute) as? String == kAXCellRole
                && strings($0).contains { $0.contains("→") }
                && (try? elementBounds($0)).map { abs($0.0.y - descenderOrigin.y) < 1 } == true
        }) else { throw Failure("No baseline cell in the row of the descender cell") }
        let (baselineOrigin, baselineSize) = try elementBounds(baseline)

        // Returns the first and last pixel rows that contain text, in points.
        func ink(_ name: String, _ origin: CGPoint, _ extent: CGSize) throws -> (Double, Double) {
            let path = "\(artifacts)/results-text-\(name).png"
            let rect = "\(Int(origin.x)),\(Int(origin.y)),\(Int(min(extent.width, 120))),\(Int(extent.height))"
            try captureRegion(rect, to: path)
            guard let data = FileManager.default.contents(atPath: path),
                  let bitmap = NSBitmapImageRep(data: data),
                  let background = bitmap.colorAt(x: 1, y: bitmap.pixelsHigh / 2)?.usingColorSpace(.deviceRGB) else {
                throw Failure("Could not read results text capture")
            }
            let rows = (0..<bitmap.pixelsHigh).filter { y in
                (0..<bitmap.pixelsWide).contains { x in
                    guard let color = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.deviceRGB) else { return false }
                    return max(abs(color.redComponent - background.redComponent),
                               abs(color.greenComponent - background.greenComponent),
                               abs(color.blueComponent - background.blueComponent)) > 0.3
                }
            }
            guard let top = rows.first, let bottom = rows.last else { throw Failure("No text in \(name) cell") }
            let scale = Double(bitmap.pixelsHigh) / Double(Int(extent.height))
            return (Double(top) / scale, Double(bottom + 1) / scale)
        }
        let (capTop, baselineBottom) = try ink("baseline", baselineOrigin, baselineSize)
        let (_, descenderBottom) = try ink("descender", descenderOrigin, descenderSize)
        let capHeight = baselineBottom - capTop
        let descent = descenderBottom - baselineBottom
        print("results text: cap height \(capHeight) pt, descent \(descent) pt")
        // A clipped cell keeps about 0.17 of the cap height below the baseline, a full one about 0.28.
        try require(descent >= 0.22 * capHeight, "Results cell clips descenders: descent \(descent) pt, cap height \(capHeight) pt")
        print("PASS: Results cells show letters below the baseline")
    }
    func stop() {
        sampleTimer?.invalidate()
        if process.isRunning {
            NSRunningApplication(processIdentifier: process.processIdentifier)?.terminate()
            let deadline = Date(timeIntervalSinceNow: 5)
            while process.isRunning && Date() < deadline { Thread.sleep(forTimeInterval: 0.1) }
            if process.isRunning { process.terminate() }
        }
        try? samples.joined(separator: "\n").write(toFile: "\(artifacts)/\(name)-resources.txt", atomically: true, encoding: .utf8)
        try? log?.close()
    }
    func savedSQL(_ sql: String, at url: URL) -> Bool {
        guard let data = try? Data(contentsOf: url),
              let workspace = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let tabs = workspace["tabs"] as? [[String: Any]] else { return false }
        return tabs.contains { $0["sql"] as? String == sql }
    }
    func testFailedSaveExit(closeWindow: Bool) throws {
        let workspace = URL(fileURLWithPath: env["QROW_DATA_DIR"]!).appendingPathComponent("workspace.json")
        let backup = workspace.deletingLastPathComponent().appendingPathComponent("workspace-before-quit.json")
        let baseline = "SELECT 'saved-before-quit' -- " + UUID().uuidString
        try fill("SQL Editor", baseline)
        var deadline = clock.now.advanced(by: .seconds(10))
        while !savedSQL(baseline, at: workspace) {
            try require(clock.now < deadline, "Initial workspace autosave did not complete")
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
        }
        try FileManager.default.moveItem(at: workspace, to: backup)
        try FileManager.default.createDirectory(at: workspace, withIntermediateDirectories: false)
        defer {
            if FileManager.default.fileExists(atPath: backup.path) {
                try? FileManager.default.removeItem(at: workspace)
                try? FileManager.default.moveItem(at: backup, to: workspace)
            }
        }
        let finalSQL = "SELECT 'unsaved 日本語😀' -- " + UUID().uuidString
        try fill("SQL Editor", finalSQL)
        func requestQuit() throws {
            if closeWindow {
                guard let window = (attribute(app, kAXWindowsAttribute) as? [AXUIElement])?.first,
                      let close = attribute(window, kAXCloseButtonAttribute) else {
                    throw Failure("Window close button is unavailable")
                }
                try click(unsafeBitCast(close, to: AXUIElement.self))
            } else {
                key(12, flags: .maskCommand)
            }
        }
        try requestQuit()
        _ = try wait("Keep Editing", timeout: 10)
        try require(process.isRunning, "Failed final save closed the application")
        try require(savedSQL(baseline, at: backup), "Failed save changed the previous workspace")
        try snapshot(closeWindow ? "failed-save-window-close" : "failed-save-quit")
        try press("Keep Editing")
        try waitGone("Keep Editing")
        guard let editor = find("SQL Editor", role: kAXTextAreaRole) ?? find("SQL Editor", role: kAXTextFieldRole) else {
            throw Failure("SQL editor is unavailable after failed save")
        }
        try require(attribute(editor, kAXValueAttribute) as? String == finalSQL, "Failed save lost the current SQL edit")
        try requestQuit()
        _ = try wait("Retry Save and Quit", timeout: 10)
        try FileManager.default.removeItem(at: workspace)
        try FileManager.default.moveItem(at: backup, to: workspace)
        try press("Retry Save and Quit")
        deadline = clock.now.advanced(by: .seconds(10))
        while process.isRunning {
            try require(clock.now < deadline, "Save retry did not finish quitting")
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
        }
        try require(savedSQL(finalSQL, at: workspace), "Quit completed without saving the final SQL edit")
        print("PASS: failed save preserves edits, Keep Editing, retry and durable save before \(closeWindow ? "window close" : "Quit")")
    }
    /// A version line reads `0.1.0`, `0.1.0 (dcc75d4fd874)`, or, for a release
    /// channel, `0.1.0-nightly.20260921.7 (dcc75d4fd874)`.
    func isVersion(_ text: String) -> Bool {
        let parts = text.split(separator: " ", maxSplits: 1, omittingEmptySubsequences: true)
        guard let first = parts.first else { return false }
        let release = first.split(separator: "-", maxSplits: 1, omittingEmptySubsequences: false)
        let numbers = release[0].split(separator: ".", omittingEmptySubsequences: false)
        guard numbers.count == 3, numbers.allSatisfy({ !$0.isEmpty && $0.allSatisfy(\.isNumber) }) else { return false }
        if release.count == 2 {
            let labels = release[1].split(separator: ".", omittingEmptySubsequences: false)
            guard labels.allSatisfy({ !$0.isEmpty && $0.allSatisfy { $0.isASCII && ($0.isLetter || $0.isNumber || $0 == "-") } })
            else { return false }
        }
        if parts.count == 1 { return true }
        let commit = parts[1].trimmingCharacters(in: CharacterSet(charactersIn: "()"))
        return commit.count == 12 && commit.allSatisfy { $0.isHexDigit && !$0.isUppercase }
    }
    /// Opens the application menu, which follows the Apple menu.
    func openApplicationMenu() throws -> AXUIElement {
        guard let bar = attribute(app, kAXMenuBarAttribute) else { throw Failure("Qrow has no menu bar") }
        let menus = (attribute(unsafeBitCast(bar, to: AXUIElement.self), kAXChildrenAttribute) as? [AXUIElement]) ?? []
        try require(menus.count > 1, "Qrow has no application menu")
        try require(AXUIElementPerformAction(menus[1], kAXPressAction as CFString) == .success, "Cannot open the application menu")
        return menus[1]
    }
    func applicationMenuItem(_ label: String, in menu: AXUIElement) throws -> AXUIElement {
        let deadline = clock.now.advanced(by: .seconds(10))
        var item: AXUIElement?
        repeat {
            item = descendants(menu).first { strings($0).contains(label) }
            if item != nil { break }
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
        } while clock.now < deadline
        guard let item else { throw Failure("The application menu has no item: \(label)") }
        return item
    }
    func selectApplicationMenuItem(_ label: String, from openedMenu: AXUIElement? = nil) throws {
        let menu = try openedMenu ?? openApplicationMenu()
        let item = try applicationMenuItem(label, in: menu)
        try require(attribute(item, kAXEnabledAttribute) as? Bool == true, "The application menu item is disabled: \(label)")
        try require(AXUIElementPerformAction(item, kAXPressAction as CFString) == .success, "Cannot select \(label)")
    }
    func checkApplicationMenuCommands(after close: String) throws {
        let menu = try openApplicationMenu()
        for label in ["About Qrow", "Settings…", "Quit Qrow"] {
            let item = try applicationMenuItem(label, in: menu)
            try require(attribute(item, kAXEnabledAttribute) as? Bool == true,
                        "\(label) is disabled after \(close) of the conversation delete dialog")
        }
        try snapshot("assistant-delete-menu-\(close)")
        // Finish menu tracking by selecting About from the menu we checked.
        try testAbout(from: menu)
    }
    func testAbout(from menu: AXUIElement? = nil) throws {
        try selectApplicationMenuItem("About Qrow", from: menu)
        let copyright = "Copyright © 2026 Vsevolod Bazhan"
        _ = try wait(copyright, timeout: 10)
        let deadline = clock.now.advanced(by: .seconds(10))
        var version: String?
        repeat {
            version = elements().flatMap(strings).first(where: isVersion)
            if version != nil { break }
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
        } while clock.now < deadline
        guard let version else { throw Failure("The About dialog does not report a version") }
        try snapshot("about")
        // Escape closes the dialog, and the menu item opens it again.
        key(53)
        try waitGone(copyright)
        try selectApplicationMenuItem("About Qrow")
        _ = try wait(copyright, timeout: 10)
        key(53)
        try waitGone(copyright)
        print("PASS: About dialog reports \(version)")
    }
    // Read the value the Settings dialog displays, not the workspace file, so
    // that the control and its binding are both checked.
    func settingValue(_ label: String) -> String? {
        let control = elements().first {
            [kAXTextFieldRole, kAXComboBoxRole, kAXPopUpButtonRole].contains(
                attribute($0, kAXRoleAttribute) as? String ?? ""
            )
                && strings($0).contains(label)
        }
        return control.flatMap { attribute($0, kAXValueAttribute) as? String }
    }
    func waitSettingValue(_ label: String, _ expected: String) throws {
        let deadline = clock.now.advanced(by: .seconds(10))
        var value: String?
        repeat {
            value = settingValue(label)
            if value == expected { return }
            try require(process.isRunning, "Qrow exited while waiting for \(label)")
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
        } while clock.now < deadline
        throw Failure("\(label) shows \(value ?? "nothing"), expected \(expected)")
    }
    /// Writes a new synthetic workspace with the UI scale and pane width at
    /// which the transcript cut off messages: a wide table reply, the last
    /// word of a message with inline code, and the end of bold text lines.
    func seedAssistantLayoutWorkspace(panelWidth: Double = 536, uiScale: Double = 1.1, theme: String? = nil) throws {
        let workspace = URL(fileURLWithPath: env["QROW_DATA_DIR"]!).appendingPathComponent("workspace.json")
        try require(!FileManager.default.fileExists(atPath: workspace.path), "The layout check needs an empty workspace directory")
        var settings: [String: Any] = [
            "ui_scale": uiScale,
            "assistant": [
                "enabled": true,
                "data_sharing_notice_version": 3,
                "codex_executable": FileManager.default.currentDirectoryPath + "/tests/desktop/fake-codex.sh",
                "panel_width": panelWidth,
            ],
        ]
        if let theme { settings["theme"] = theme }
        let data = try JSONSerialization.data(withJSONObject: [
            "version": 3, "settings": settings, "profiles": [], "tabs": [], "active_tab": 0,
        ])
        try data.write(to: workspace)
    }
    /// A selection in a user message bubble shows in One Dark, and the selected
    /// text keeps its color. A selection painted over the glyphs dimmed them,
    /// and One Dark's earlier selection color matched the bubble.
    func testAssistantSelection() throws {
        key(38, flags: .maskCommand) // Cmd+J opens the docked assistant.
        _ = try wait("Model: Synthetic Model", timeout: 20)
        try fill("Assistant Message", "Count the weekend before last.")
        try press("Send")
        _ = try wait("I can help with this query", timeout: 20)
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.5))
        let message = try wait("You: Count the weekend before last.", timeout: 5)
        let (origin, extent) = try elementBounds(message)

        // The message text covers the capture, so its most frequent color is
        // the surface behind the text, and its lightest color is the text.
        func capture(_ name: String) throws -> (surface: NSColor, text: NSColor) {
            let path = "\(artifacts)/\(name).png"
            try captureRegion("\(Int(origin.x)),\(Int(origin.y)),\(Int(extent.width)),\(Int(extent.height))", to: path)
            guard let data = FileManager.default.contents(atPath: path),
                  let bitmap = NSBitmapImageRep(data: data) else {
                throw Failure("Could not read the \(name) capture")
            }
            var counts: [Int: Int] = [:]
            var text: NSColor?
            for y in 0..<bitmap.pixelsHigh {
                for x in 0..<bitmap.pixelsWide {
                    guard let color = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.deviceRGB) else { continue }
                    let rgb = [color.redComponent, color.greenComponent, color.blueComponent].map { Int(($0 * 255).rounded()) }
                    counts[rgb[0] << 16 | rgb[1] << 8 | rgb[2], default: 0] += 1
                    if text.map({ color.redComponent + color.greenComponent + color.blueComponent
                        > $0.redComponent + $0.greenComponent + $0.blueComponent }) ?? true {
                        text = color
                    }
                }
            }
            guard let surface = counts.max(by: { $0.value < $1.value })?.key, let text else {
                throw Failure("The \(name) capture is empty")
            }
            return (NSColor(deviceRed: CGFloat(surface >> 16) / 255,
                            green: CGFloat(surface >> 8 & 255) / 255,
                            blue: CGFloat(surface & 255) / 255,
                            alpha: 1), text)
        }
        func distance(_ a: NSColor, _ b: NSColor) -> CGFloat {
            max(abs(a.redComponent - b.redComponent),
                abs(a.greenComponent - b.greenComponent),
                abs(a.blueComponent - b.blueComponent))
        }

        let before = try capture("assistant-selection-before")
        // A triple click selects the line of the message.
        let center = CGPoint(x: origin.x + extent.width / 2, y: origin.y + extent.height / 2)
        for clickState in 1...3 {
            for eventType in [CGEventType.leftMouseDown, .leftMouseUp] {
                let event = CGEvent(mouseEventSource: nil, mouseType: eventType, mouseCursorPosition: center, mouseButton: .left)!
                event.setIntegerValueField(.mouseEventClickState, value: Int64(clickState))
                event.post(tap: .cghidEventTap)
            }
        }
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.5))
        let after = try capture("assistant-selection-after")
        try snapshot("assistant-selected-message")
        try require(distance(after.surface, before.surface) > 0.06,
                    "The selection does not show on the user message bubble: \(before.surface) and \(after.surface)")
        try require(distance(after.text, before.text) < 0.05,
                    "The selection changes the color of the selected text from \(before.text) to \(after.text)")
        print("PASS: A selection in a user message shows in One Dark and keeps the text color")
    }
    /// At this width the first message fits on one line, and the second
    /// message wraps. If a layout at the bubble's own width breaks a line
    /// again, the bubble keeps the height of the first layout and hides the
    /// last line, and the message extends below the bubble. The insets are
    /// checked on the second message, because the second reply scrolls the
    /// first message out of the transcript.
    func checkInlineCodeMessages() throws {
        _ = try checkInlineCodeMessage(
            "How many tables are in `sandbox_vbazhan` schema?", name: "assistant-inline-code-message")
        let message = try checkInlineCodeMessage(
            "When did the latest vacuum complete on `integrations.bookings`?", name: "assistant-wrapped-inline-code-message")
        try checkMessageInsets(message)
    }
    /// Sends `text`. Left of the text, the bottom of the message must have the
    /// bubble color of its top. Returns the label of the message.
    func checkInlineCodeMessage(_ text: String, name: String) throws -> String {
        try fill("Assistant Message", text)
        try press("Send")
        let label = "You: \(text)"
        _ = try wait(label, timeout: 20)
        _ = try wait("I can help with this query", timeout: 20)
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.5))
        // The reply rebuilds the transcript, so an element found before it can
        // have no bounds. Find the message again.
        let (origin, extent) = try elementBounds(try wait(label, timeout: 5))
        try snapshot(name)
        let path = "\(artifacts)/\(name)-edge.png"
        let rect = "\(Int(origin.x) + 1),\(Int(origin.y)),1,\(Int(extent.height))"
        try captureRegion(rect, to: path)
        guard let data = FileManager.default.contents(atPath: path),
              let bitmap = NSBitmapImageRep(data: data),
              let top = bitmap.colorAt(x: 0, y: 2)?.usingColorSpace(.deviceRGB),
              let bottom = bitmap.colorAt(x: 0, y: bitmap.pixelsHigh - 3)?.usingColorSpace(.deviceRGB) else {
            throw Failure("Could not read the message edge capture")
        }
        let difference = max(abs(top.redComponent - bottom.redComponent),
                             abs(top.greenComponent - bottom.greenComponent),
                             abs(top.blueComponent - bottom.blueComponent))
        try require(difference < 0.02, "The message \(text) extends below its bubble")
        return label
    }
    /// A line that is wider than the reply is clipped at the reply's right
    /// edge, so glyphs touch that edge. A line that wraps correctly ends
    /// before it. The reply is drawn on the transcript background.
    func checkBoldReply() throws {
        try fill("Assistant Message", "Show a bold reply")
        try press("Send")
        let label = "Assistant: There were **44,266,382 distinct searches**"
        _ = try wait(label, timeout: 20)
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.5))
        let (origin, extent) = try elementBounds(try wait(label, timeout: 5))
        let (composerOrigin, _) = try elementBounds(try waitInput("Assistant Message"))
        try snapshot("assistant-bold-reply")
        let bottom = min(origin.y + extent.height, composerOrigin.y)
        try require(bottom - origin.y > 100, "The bold reply is not on screen")
        let path = "\(artifacts)/assistant-bold-reply-edge.png"
        let rect = "\(Int(origin.x - 4)),\(Int(origin.y)),\(Int(extent.width) + 4),\(Int(bottom - origin.y))"
        try captureRegion(rect, to: path)
        guard let data = FileManager.default.contents(atPath: path),
              let bitmap = NSBitmapImageRep(data: data),
              let background = bitmap.colorAt(x: 0, y: 0)?.usingColorSpace(.deviceRGB) else {
            throw Failure("Could not read the bold reply capture")
        }
        let scale = CGFloat(bitmap.pixelsWide) / (CGFloat(Int(extent.width)) + 4)
        let edge = Int(scale * 2)
        let inked = (bitmap.pixelsWide - edge..<bitmap.pixelsWide).contains { x in
            (0..<bitmap.pixelsHigh).contains { y in
                guard let color = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.deviceRGB) else { return false }
                return max(abs(color.redComponent - background.redComponent),
                           abs(color.greenComponent - background.greenComponent),
                           abs(color.blueComponent - background.blueComponent)) > 0.1
            }
        }
        try require(!inked, "Text in the bold reply reaches the right edge of the reply")
    }
    /// The composer has the same space above the message field as below the
    /// Send button. Scan a column in the right padding of the composer, from
    /// the border above it to the status bar below it.
    func checkComposerPadding() throws {
        let (composerOrigin, composerSize) = try elementBounds(try waitInput("Assistant Message"))
        let (sendOrigin, sendSize) = try elementBounds(try wait("Send"))
        let top = Int(composerOrigin.y) - 24
        let height = Int(sendOrigin.y + sendSize.height) + 24 - top
        let path = "\(artifacts)/assistant-composer-padding.png"
        try captureRegion("\(Int(composerOrigin.x + composerSize.width) + 4),\(top),1,\(height)", to: path)
        guard let data = FileManager.default.contents(atPath: path),
              let bitmap = NSBitmapImageRep(data: data) else {
            throw Failure("Could not read the composer padding capture")
        }
        let scale = CGFloat(bitmap.pixelsHigh) / CGFloat(height)
        func row(_ y: CGFloat) -> Int { Int(((y - CGFloat(top)) * scale).rounded()) }
        guard let background = bitmap.colorAt(x: 0, y: row(composerOrigin.y + 4))?.usingColorSpace(.deviceRGB) else {
            throw Failure("Could not read the composer background")
        }
        func differs(_ y: Int) -> Bool {
            guard let color = bitmap.colorAt(x: 0, y: y)?.usingColorSpace(.deviceRGB) else { return false }
            return max(abs(color.redComponent - background.redComponent),
                       abs(color.greenComponent - background.greenComponent),
                       abs(color.blueComponent - background.blueComponent)) > 0.02
        }
        guard let border = (0..<row(composerOrigin.y)).last(where: differs),
              let bottom = (row(sendOrigin.y + sendSize.height)..<bitmap.pixelsHigh).first(where: differs) else {
            throw Failure("Could not find the composer edges in the padding capture")
        }
        let above = (CGFloat(row(composerOrigin.y) - border - 1)) / scale
        let below = (CGFloat(bottom - row(sendOrigin.y + sendSize.height))) / scale
        try require(abs(above - below) <= 1, "The composer has \(above) points above the message field and \(below) points below Send")
    }
    /// The transcript and the composer share one horizontal inset. The reply
    /// text starts at the left edge of the composer, and the user bubble ends
    /// at its right edge. A reply bubble without a visible surface adds its
    /// padding on the left only.
    func checkMessageInsets(_ message: String) throws {
        let reply = try wait("Assistant: **I can help with this query", timeout: 5)
        let (composerOrigin, composerSize) = try elementBounds(try waitInput("Assistant Message"))
        let (replyOrigin, _) = try elementBounds(reply)
        try require(abs(replyOrigin.x - composerOrigin.x) <= 1,
                    "The assistant reply starts \(replyOrigin.x - composerOrigin.x) points right of the composer")

        // Scan a row through the user bubble from the transcript background
        // on its left, and find the last point that has another color.
        let (origin, extent) = try elementBounds(try wait(message, timeout: 5))
        let left = Int(composerOrigin.x) + 2
        let width = Int(composerOrigin.x + composerSize.width) + 6 - left
        let path = "\(artifacts)/assistant-message-insets.png"
        try captureRegion("\(left),\(Int(origin.y + extent.height / 2)),\(width),1", to: path)
        guard let data = FileManager.default.contents(atPath: path),
              let bitmap = NSBitmapImageRep(data: data),
              let background = bitmap.colorAt(x: 0, y: 0)?.usingColorSpace(.deviceRGB) else {
            throw Failure("Could not read the message inset capture")
        }
        let scale = CGFloat(bitmap.pixelsWide) / CGFloat(width)
        let bubbleEnd = (0..<bitmap.pixelsWide).last { x in
            guard let color = bitmap.colorAt(x: x, y: 0)?.usingColorSpace(.deviceRGB) else { return false }
            return max(abs(color.redComponent - background.redComponent),
                       abs(color.greenComponent - background.greenComponent),
                       abs(color.blueComponent - background.blueComponent)) > 0.02
        }
        guard let bubbleEnd else { throw Failure("Could not find the user bubble in the inset capture") }
        let bubbleRight = CGFloat(left) + CGFloat(bubbleEnd + 1) / scale
        let composerRight = composerOrigin.x + composerSize.width
        try require(abs(bubbleRight - composerRight) <= 1,
                    "The user bubble ends \(composerRight - bubbleRight) points left of the composer")
    }
    /// The application menu opens Settings. The headless UI tests check the
    /// settings themselves.
    func testSettingsMenu() throws {
        try selectApplicationMenuItem("Settings…")
        try waitSettingValue("Theme", "System")
        try press("Save")
        try waitGone("UI Scale")
        print("PASS: The application menu opens and closes Settings")
    }
    func testAssistantDeleteMenu() throws {
        key(38, flags: .maskCommand)
        _ = try wait("Model: Synthetic Model", timeout: 20)
        if find("Search Conversations") != nil {
            try press("Toggle Conversation List")
            try waitGone("Search Conversations", timeout: 5)
        }
        try fill("Assistant Message", "Explain SELECT 1")
        try press("Send")
        _ = try wait("I can help with this query", timeout: 20)
        for close in ["cancel", "escape", "delete"] {
            // The dropdown opens on pointer input around the button.
            try press("Conversation Actions", pointer: true)
            try click(try wait("Delete…"))
            _ = try waitExact("Delete", timeout: 10, role: kAXButtonRole)
            switch close {
            case "cancel": try press("Cancel", pointer: true)
            case "escape": key(53)
            default: try press("Delete", pointer: true)
            }
            try waitGone("Cancel", role: kAXButtonRole)
            // No click in the workspace may repair focus before this check.
            try checkApplicationMenuCommands(after: close)
        }
        try testSettingsMenu()
        print("PASS: About Qrow, Settings, and Quit Qrow stay enabled after Delete, Cancel, and Escape")
    }
    /// Pixel checks of assistant messages at the scale and width at which the
    /// transcript cut them off. The headless UI tests check their geometry.
    func testAssistantLayout() throws {
        key(38, flags: .maskCommand) // Cmd+J opens the docked assistant.
        _ = try wait("Model: Synthetic Model", timeout: 20)
        if find("Search Conversations") != nil {
            try press("Toggle Conversation List")
            try waitGone("Search Conversations", timeout: 5)
        }
        try checkInlineCodeMessages()
        try checkBoldReply()
        try checkComposerPadding()
        print("PASS: Assistant messages with inline code show every line, bold text wraps inside the reply, and the composer padding is even")
    }
    /// The packaged app on the desktop: the menu bar, a connection whose
    /// password goes to the real Keychain, a real query, and a quit whose
    /// final save fails. The headless UI and E2E suites check the rest.
    func test() throws {
        try start()
        try testAbout()
        try testSettingsMenu()
        try press("New Connection")
        for (label, value) in [("Name", "Qrow E2E"), ("Host", "127.0.0.1"),
                               ("Port", env["QROW_E2E_PORT"]!), ("Username", "qrow"),
                               ("Password", "qrow-test-password"), ("Initial database", "default")] {
            try fill(label, value)
        }
        try press("Save")
        _ = try wait("Qrow E2E")
        try waitGone("Cancel")
        // The worker reads the password from Keychain for the session.
        try press("Qrow E2E")
        try query("SELECT 'qrow-ui-connected' AS result")
        _ = try wait("qrow-ui-connected", role: kAXCellRole)
        try snapshot("connected")
        try testFailedSaveExit(closeWindow: false)
        print("PASS: Menu bar dialogs, a Keychain password for a real query, and a failed save before Quit")
    }
}

do {
    try require(AXIsProcessTrusted(), "Native UI tests require Accessibility permission for the driver/terminal. No UI tests ran.")
    try require(CGPreflightScreenCaptureAccess(), "Native UI tests require Screen Recording permission for failure screenshots. No UI tests ran.")
    if !CommandLine.arguments.contains("--preflight") {
        let driver = Driver()
        do {
            if CommandLine.arguments.contains("--assistant-layout-only") {
                try driver.seedAssistantLayoutWorkspace()
                try driver.start()
                try driver.testAssistantLayout()
            } else if CommandLine.arguments.contains("--assistant-selection-only") {
                try driver.seedAssistantLayoutWorkspace(theme: "One Dark")
                try driver.start()
                try driver.testAssistantSelection()
            } else if CommandLine.arguments.contains("--assistant-delete-menu-only") {
                try driver.seedAssistantLayoutWorkspace(uiScale: 1)
                try driver.start()
                try driver.testAssistantDeleteMenu()
            } else if CommandLine.arguments.contains("--editor-highlight-only") {
                try driver.start()
                try driver.testEditorHighlight()
            } else if CommandLine.arguments.contains("--results-text-only") {
                try driver.start()
                try driver.testResultsText()
            } else if CommandLine.arguments.contains("--window-close-only") {
                try driver.start()
                try driver.testFailedSaveExit(closeWindow: true)
            } else {
                try driver.test()
            }
            driver.stop()
        } catch {
            if driver.app != nil { try? driver.snapshot("failure") }
            driver.stop()
            throw error
        }
    }
} catch {
    fputs("\(error)\n", stderr)
    exit(1)
}
