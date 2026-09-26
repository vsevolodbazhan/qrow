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
    guard let position, let size else { throw Failure("Element has no bounds") }
    var point = CGPoint.zero
    var extent = CGSize.zero
    try require(CFGetTypeID(position) == AXValueGetTypeID() && CFGetTypeID(size) == AXValueGetTypeID(), "Invalid element bounds")
    AXValueGetValue(unsafeBitCast(position, to: AXValue.self), .cgPoint, &point)
    AXValueGetValue(unsafeBitCast(size, to: AXValue.self), .cgSize, &extent)
    return (point, extent)
}
func click(_ element: AXUIElement) throws {
    // Dialog accessibility nodes appear before their opening animation settles.
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.3))
    let (point, extent) = try elementBounds(element)
    print("Click \(strings(element)): \(point) \(extent)")
    var clickPoint = point
    clickPoint.x += extent.width / 2
    clickPoint.y += extent.height / 2
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
    var clickPoint = point
    clickPoint.x += extent.width / 2
    clickPoint.y += extent.height / 2
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
    func waitGone(_ label: String, timeout: Double = 10) throws {
        let deadline = clock.now.advanced(by: .seconds(timeout))
        while find(label) != nil {
            try require(clock.now < deadline, "Old UI state remained visible: \(label)")
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.05))
        }
    }
    func contextMenu(_ label: String, exact: Bool = false, role: String? = nil) throws {
        let actions = ["Edit Connection…", "Duplicate", "Delete", "Rename…", "Move to Connection…"]
        for attempt in 0..<3 {
            if actions.contains(where: { find($0) != nil }) { return }
            if attempt > 0 { key(53) }
            let target = exact
                ? try waitExact(label, timeout: 10, role: role)
                : try wait(label, timeout: 10, role: role)
            try rightClick(target)
            let deadline = clock.now.advanced(by: .seconds(3))
            repeat {
                if actions.contains(where: { find($0) != nil }) { return }
                RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.05))
            } while clock.now < deadline
        }
        throw Failure("Context menu did not open for \(label)")
    }
    func press(_ label: String) throws {
        let deadline = clock.now.advanced(by: .seconds(150))
        repeat {
            let control = find(label, role: kAXButtonRole) ?? find(label, role: kAXCheckBoxRole)
            if let control, attribute(control, kAXEnabledAttribute) as? Bool != false {
                // Prefer the control's native accessibility action. GPUI Kit
                // alert buttons can be present in the accessibility tree
                // before their hit-test surface is ready for a pointer click.
                if AXUIElementPerformAction(control, kAXPressAction as CFString) != .success {
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
    func selectPopup(_ label: String, _ option: String) throws {
        let popup = try wait(label, role: kAXPopUpButtonRole)
        let current = strings(popup).first { $0 != label }
        try activate(popup)
        // The accessibility click schedules the deferred popup and transfers
        // focus to its list. Do not send navigation keys until that frame is
        // published; the delay varies with display scaling.
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.3))
        if current != option {
            try require(
                ["Disconnect after", "Keep connected"].contains(option),
                "Unknown idle behavior: \(option)"
            )
            key(option == "Keep connected" ? 125 : 126)
            key(36)
        } else {
            key(53)
        }
        let deadline = clock.now.advanced(by: .seconds(10))
        repeat {
            if let popup = find(label, role: kAXPopUpButtonRole), strings(popup).contains(option) {
                return
            }
            try require(process.isRunning, "Qrow exited while selecting \(option)")
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
        } while clock.now < deadline
        throw Failure("Popup \(label) did not select \(option)")
    }
    func newTab(_ expected: String) throws {
        // A successful click can return before GPUI publishes the new tab to
        // the accessibility tree. Confirm the tab before sending the next
        // input, and retry only if the tab was not created.
        for _ in 0..<2 {
            if findExact(expected, role: kAXRadioButtonRole) != nil { return }
            try press("New Tab")
            let deadline = clock.now.advanced(by: .seconds(5))
            repeat {
                if findExact(expected, role: kAXRadioButtonRole) != nil { return }
                try require(process.isRunning, "Qrow exited while waiting for \(expected)")
                RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
            } while clock.now < deadline
        }
        throw Failure("Timed out waiting for \(expected) after creating a tab")
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
    func selectConnection(_ name: String) throws {
        try press(name)
        // Button styling does not expose selection through accessibility.
        // Check the isolated workspace to catch a click that was ignored.
        let workspaceURL = URL(fileURLWithPath: env["QROW_DATA_DIR"]!).appendingPathComponent("workspace.json")
        let deadline = clock.now.advanced(by: .seconds(5))
        repeat {
            if let data = try? Data(contentsOf: workspaceURL),
               let workspace = try JSONSerialization.jsonObject(with: data) as? [String: Any],
               let profiles = workspace["profiles"] as? [[String: Any]],
               let tabs = workspace["tabs"] as? [[String: Any]],
               let active = workspace["active_tab"] as? Int,
               tabs.indices.contains(active),
               let selected = tabs[active]["profile"] as? String,
               profiles.contains(where: { $0["id"] as? String == selected && $0["name"] as? String == name }) {
                return
            }
            try require(process.isRunning, "Qrow exited while selecting \(name)")
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
        } while clock.now < deadline
        throw Failure("Connection selection was not saved: \(name)")
    }
    func snapshot(_ name: String) throws {
        let text = elements().map { "\(attribute($0, kAXRoleAttribute) ?? "?" as CFString) \(strings($0))" }.joined(separator: "\n")
        try text.write(toFile: "\(artifacts)/\(name)-accessibility.txt", atomically: true, encoding: .utf8)
        let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
        guard let window = windows.first(where: {
            $0[kCGWindowOwnerPID as String] as? Int32 == process.processIdentifier && $0[kCGWindowLayer as String] as? Int == 0
        }), let number = window[kCGWindowNumber as String] as? UInt32 else { throw Failure("No Qrow window to capture") }
        _ = try command(["screencapture", "-x", "-l", "\(number)", "\(artifacts)/\(name).png"])
    }
    func start() throws {
        let bundle = env["QROW_E2E_BUNDLE"]!
        let logURL = URL(fileURLWithPath: "\(artifacts)/\(name).log")
        FileManager.default.createFile(atPath: logURL.path, contents: nil)
        log = try FileHandle(forWritingTo: logURL)
        process.executableURL = URL(fileURLWithPath: "\(bundle)/Contents/MacOS/qrow")
        if CommandLine.arguments.contains("--editor-highlight-only") {
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
        _ = try wait(CommandLine.arguments.contains("--editor-highlight-only") ? "SQL Editor" : "New Connection", timeout: 20)
        samples.append("launch_to_accessible_new_connection_seconds=\(started.duration(to: clock.now))")
        sampleTimer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
            guard let self else { return }
            if let sample = try? command(["ps", "-o", "rss=,%cpu=", "-p", "\(self.process.processIdentifier)"]) {
                self.samples.append("\(Date().timeIntervalSince1970) \(sample)")
            }
        }
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
        _ = try command(["screencapture", "-x", "-R", rect, path])
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
    /// A version line reads `0.1.0` or `0.1.0 (dcc75d4fd874)`.
    func isVersion(_ text: String) -> Bool {
        let parts = text.split(separator: " ", maxSplits: 1, omittingEmptySubsequences: true)
        guard let first = parts.first else { return false }
        let numbers = first.split(separator: ".", omittingEmptySubsequences: false)
        guard numbers.count == 3, numbers.allSatisfy({ !$0.isEmpty && $0.allSatisfy(\.isNumber) }) else { return false }
        if parts.count == 1 { return true }
        let commit = parts[1].trimmingCharacters(in: CharacterSet(charactersIn: "()"))
        return commit.count == 12 && commit.allSatisfy { $0.isHexDigit && !$0.isUppercase }
    }
    /// Select an item of the application menu, which accessibility exposes
    /// after the Apple menu.
    func selectApplicationMenuItem(_ label: String) throws {
        guard let bar = attribute(app, kAXMenuBarAttribute) else { throw Failure("Qrow has no menu bar") }
        let menus = (attribute(unsafeBitCast(bar, to: AXUIElement.self), kAXChildrenAttribute) as? [AXUIElement]) ?? []
        try require(menus.count > 1, "Qrow has no application menu")
        try require(AXUIElementPerformAction(menus[1], kAXPressAction as CFString) == .success, "Cannot open the application menu")
        let deadline = clock.now.advanced(by: .seconds(10))
        var item: AXUIElement?
        repeat {
            item = descendants(menus[1]).first { strings($0).contains(label) }
            if item != nil { break }
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
        } while clock.now < deadline
        guard let item else { throw Failure("The application menu has no item: \(label)") }
        try require(AXUIElementPerformAction(item, kAXPressAction as CFString) == .success, "Cannot select \(label)")
    }
    func testAbout() throws {
        try selectApplicationMenuItem("About Qrow")
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
            attribute($0, kAXRoleAttribute) as? String == kAXTextFieldRole && strings($0).contains(label)
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
    func waitSavedAssistantFont(_ expected: String) throws {
        let workspace = URL(fileURLWithPath: env["QROW_DATA_DIR"]!).appendingPathComponent("workspace.json")
        let deadline = clock.now.advanced(by: .seconds(10))
        repeat {
            if let data = try? Data(contentsOf: workspace),
               let content = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
               let settings = content["settings"] as? [String: Any],
               settings["assistant_font_family"] as? String == expected { return }
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
        } while clock.now < deadline
        throw Failure("Assistant font was not saved as \(expected)")
    }
    func waitStaleConversationRemoved() throws {
        let workspace = URL(fileURLWithPath: env["QROW_DATA_DIR"]!).appendingPathComponent("workspace.json")
        let deadline = clock.now.advanced(by: .seconds(10))
        repeat {
            if let data = try? Data(contentsOf: workspace),
               let content = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
               let assistant = content["assistant"] as? [String: Any],
               let conversations = assistant["conversations"] as? [[String: Any]],
               // Qrow opens a new conversation but saves it only after its first message.
               assistant["selected_thread"] is NSNull,
               conversations.isEmpty { return }
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
        } while clock.now < deadline
        throw Failure("Missing Codex conversation stayed in the Qrow workspace")
    }
    func testAssistantRestart() throws {
        key(38, flags: .maskCommand) // Cmd+J opens the docked assistant.
        _ = try wait("Assistant model: Synthetic Model", timeout: 20)
        if find("Assistant message") == nil {
            try press("Back to conversation")
        }
        // The previous process left an unsent conversation open.
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 1))
        try require(find("Codex cannot find this conversation") == nil, "An unsent conversation was restored after restart")
        try fill("Assistant message", "Explain `SELECT 1` after restart")
        try press("Send")
        _ = try wait("I can help with this query", timeout: 20)
        try snapshot("assistant-after-restart")
        print("PASS: An unsent Assistant conversation does not fail after restart")
    }
    func selectFont(_ label: String, _ family: String) throws {
        var popup = try wait(label, role: kAXPopUpButtonRole)
        scrollIntoView(popup)
        popup = try wait(label, role: kAXPopUpButtonRole)
        try click(popup)
        let searchDeadline = clock.now.advanced(by: .seconds(5))
        var search: AXUIElement?
        repeat {
            let fields = elements().filter {
                attribute($0, kAXRoleAttribute) as? String == kAXTextFieldRole
                    && strings($0).contains("Search...")
            }
            if fields.count > 1 { search = fields.last; break }
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.05))
        } while clock.now < searchDeadline
        guard let search else {
            throw Failure("Font search did not open: \(label)")
        }
        try require(
            AXUIElementSetAttributeValue(search, kAXValueAttribute as CFString, family as CFString) == .success,
            "Could not search \(label) for \(family)"
        )
        // GPUI's virtual font rows do not expose AX bounds. With the exact
        // family name filtered to one row, click the row below the search box.
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.3))
        let (origin, size) = try elementBounds(search)
        clickPoint(CGPoint(x: origin.x + size.width / 2, y: origin.y + size.height + 16))
        let valueDeadline = clock.now.advanced(by: .seconds(5))
        repeat {
            if let popup = find(label, role: kAXPopUpButtonRole),
               attribute(popup, kAXValueAttribute) as? String == family { return }
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.05))
        } while clock.now < valueDeadline
        throw Failure("\(label) did not select \(family)")
    }
    func testSettings() throws {
        try selectApplicationMenuItem("Settings…")
        // Assistant typography is a section within Appearance.
        try waitSettingValue("UI Scale", "100")
        try selectFont("UI Font Family", "Menlo")
        try selectFont("UI Font Family", "System Font")
        // Selecting Assistant must reveal its font controls in Appearance.
        try snapshot("settings-before-assistant")
        let (search, _) = try elementBounds(waitInput("Search..."))
        clickPoint(CGPoint(x: search.x + 42, y: search.y + 100))
        try snapshot("settings-after-assistant")
        try waitSettingValue("Assistant Font Size", "14")
        let (assistantFont, _) = try elementBounds(wait("Assistant Font Family", role: kAXPopUpButtonRole))
        let (save, _) = try elementBounds(wait("Save", role: kAXButtonRole))
        try require(assistantFont.y < save.y, "Assistant settings opened below the visible page")
        try press("Increase Assistant Font Size")
        try waitSettingValue("Assistant Font Size", "15")
        try selectFont("Assistant Font Family", "Menlo")
        try waitSavedAssistantFont("Menlo")
        try selectFont("Assistant Font Family", "System Font")
        try waitSavedAssistantFont(".SystemUIFont")
        try snapshot("settings")
        try fill("Search...", "editor")
        try waitSettingValue("Editor Font Size", "13")
        try press("Increase Editor Font Size")
        try waitSettingValue("Editor Font Size", "14")
        try selectFont("Editor Font Family", "System Font")
        try fill("Search...", "logs")
        try waitSettingValue("Logs Font Size", "13")
        try selectFont("Logs Font Family", "System Font")
        try fill("Search...", "")
        try press("Restore defaults")
        try fill("Search...", "assistant")
        try waitSettingValue("Assistant Font Size", "14")
        try fill("Search...", "editor")
        try waitSettingValue("Editor Font Size", "13")
        try press("Save")
        try waitGone("UI Scale")
        print("PASS: Settings applies all font pickers, changes sizes, and restores defaults")
    }
    func verifyAssistantFontSwitch() throws {
        for (choice, stored, screenshot) in [("System Font", ".SystemUIFont", "assistant-system-font"),
                                              ("Menlo", "Menlo", "assistant-menlo-font")] {
            try selectApplicationMenuItem("Settings…")
            let (search, _) = try elementBounds(waitInput("Search..."))
            clickPoint(CGPoint(x: search.x + 42, y: search.y + 100))
            _ = try wait("Assistant Font Family", role: kAXPopUpButtonRole)
            try selectFont("Assistant Font Family", choice)
            try press("Save")
            try waitSavedAssistantFont(stored)
            try snapshot(screenshot)
        }
    }
    func testAssistant(fontOnly: Bool = false) throws {
        try selectApplicationMenuItem("Settings…")
        // GPUI Kit does not publish the settings sidebar labels through AX.
        // Anchor the pointer to the visible search field in the same pane.
        for _ in 0..<3 {
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.3))
            let (search, _) = try elementBounds(waitInput("Search..."))
            clickPoint(CGPoint(x: search.x + 42, y: search.y + 196))
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.3))
            if find("Codex executable") != nil { break }
        }
        _ = try wait("Codex executable", timeout: 5)
        try fill("Codex executable", FileManager.default.currentDirectoryPath + "/tests/e2e/native/fake-codex.sh")
        try press("Enable AI assistant")
        try press("Enable assistant")
        for _ in 0..<3 {
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.3))
            try press("Save")
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.3))
            if find("Codex executable") == nil { break }
        }
        try waitGone("Codex executable", timeout: 5)
        key(38, flags: .maskCommand) // Cmd+J opens the docked assistant.
        _ = try wait("Toggle Assistant")
        try require(find("New Connection") != nil, "Opening the assistant hid the Connections sidebar")
        if fontOnly {
            _ = try wait("Assistant model: Synthetic Model", timeout: 20)
            if find("Search conversations") == nil {
                try press("Toggle conversation list")
            }
            let search = try wait("Search conversations")
            let toggle = try wait("Toggle conversation list")
            let (searchPosition, searchSize) = try elementBounds(search)
            let (togglePosition, toggleSize) = try elementBounds(toggle)
            try require(searchSize.height <= toggleSize.height + 1, "Conversation search input is taller than the header control")
            let searchCenter = searchPosition.y + searchSize.height / 2
            let toggleCenter = togglePosition.y + toggleSize.height / 2
            try require(abs(searchCenter - toggleCenter) <= 2, "Conversation search input is not aligned with the header")
            try snapshot("assistant-conversation-search-spacing")
            if find("Assistant message") == nil {
                try press("Back to conversation")
            }
            try fill("Assistant message", "Explain `SELECT 1` in one sentence")
            try press("Send")
            _ = try wait("I can help with this query", timeout: 20)
            try snapshot("assistant-before-font-change")
            try verifyAssistantFontSwitch()
            try click(wait("Conversation actions", role: kAXButtonRole))
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.2))
            key(125) // Down selects Rename.
            key(125) // Down selects Delete.
            key(36)  // Return opens the confirmation.
            try press("Delete")
            try waitStaleConversationRemoved()
            print("PASS: Assistant fonts persist and a missing Codex conversation can be deleted")
            return
        }
        let startingModel = try wait("Assistant model unavailable", timeout: 5, role: kAXButtonRole)
        try require(attribute(startingModel, kAXEnabledAttribute) as? Bool == false, "Model was enabled while Codex started")
        for label in ["Assistant reasoning unavailable", "Assistant service tier unavailable", "Send · Ask"] {
            let control = try wait(label, timeout: 5, role: kAXButtonRole)
            try require(attribute(control, kAXEnabledAttribute) as? Bool == false, "\(label) was enabled while Codex started")
        }
        try require(find("Starting Codex") == nil, "Startup status appeared above the message field")
        _ = try wait("Toggle Assistant")
        _ = try wait("New Conversation")
        _ = try wait("Toggle conversation list")
        _ = try wait("Conversation actions")
        let listInitiallyVisible = find("Search conversations") != nil
        try press("Toggle conversation list")
        if listInitiallyVisible {
            try waitGone("Search conversations")
            try press("Toggle conversation list")
        } else {
            _ = try wait("Search conversations")
            try press("Back to conversation")
        }
        let readyModel = try wait("Assistant model: Synthetic Model", timeout: 20)
        try require(attribute(readyModel, kAXEnabledAttribute) as? Bool != false, "Model stayed disabled after Codex started")
        _ = try wait("Ask", role: kAXPopUpButtonRole)
        try fill("Assistant message", "Keep this draft")
        try press("Run")
        let draft = attribute(try waitInput("Assistant message"), kAXValueAttribute) as? String
        try require(draft == "Keep this draft", "Toolbar Run sent the assistant draft")
        try fill("Assistant message", "Help me understand how `SELECT 1` behaves in the currently selected query tab and explain its result")
        key(36, flags: .maskCommand) // Cmd+Enter sends only in the composer.
        _ = try wait("I can help with this query", timeout: 20)
        try snapshot("assistant-markdown")
        try verifyAssistantFontSwitch()
        try fill("Assistant message", "Write SELECT 1 into this tab")
        try press("Send")
        _ = try wait("I updated the SQL.", timeout: 20)
        try waitInputValue("SQL Editor", "SELECT 1")
        // Tool calls are full-width cards that start collapsed.
        let toolCard = try waitExact("Tool call: Edit query", timeout: 5)
        try require(find("Arguments:") == nil, "The tool call opened expanded")
        try require(find("Tab: Query 1") == nil, "The collapsed tool call showed its tab")
        let (_, toolSize) = try elementBounds(toolCard)
        let (_, composerSize) = try elementBounds(try waitInput("Assistant message"))
        try require(toolSize.width >= composerSize.width * 0.9, "The tool call card did not use the full transcript width")
        try snapshot("assistant")
        try activate(toolCard)
        _ = try wait("Arguments:", timeout: 5)
        _ = try wait("Tab: Query 1", timeout: 5)
        try snapshot("assistant-tool-expanded")
        try activate(try waitExact("Tool call: Edit query", timeout: 5))
        try waitGone("Arguments:", timeout: 5)
        let editor = try waitInput("SQL Editor")
        let (editorPosition, _) = try elementBounds(editor)
        clickPoint(CGPoint(x: editorPosition.x + 80, y: editorPosition.y + 20))
        try require(
            AXUIElementSetAttributeValue(editor, kAXFocusedAttribute as CFString, kCFBooleanTrue) == .success,
            "Could not focus SQL Editor for Undo",
        )
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.2))
        key(0) // A normal user edit must not merge with the assistant edit.
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.2))
        key(6, flags: .maskCommand)
        try waitInputValue("SQL Editor", "SELECT 1")
        key(6, flags: .maskCommand) // The assistant edit is one Undo step.
        try waitInputValue("SQL Editor", "")
        try fill("Assistant message", "Show many lines")
        try press("Send")
        _ = try wait("Line 40", timeout: 20)
        try scrollUpAbove(waitInput("Assistant message"))
        _ = try wait("Jump to latest", timeout: 5)
        try snapshot("assistant-scrolled")
        try press("Jump to latest")
        try waitGone("Jump to latest", timeout: 5)
        // Revisit the wrapped user message after the transcript has been scrolled.
        for _ in 0..<4 { try scrollUpAbove(waitInput("Assistant message")) }
        try snapshot("assistant-wrapped-after-scroll")
        try press("Jump to latest")
        try waitGone("Jump to latest", timeout: 5)
        try scrollUpAbove(waitInput("Assistant message"))
        _ = try wait("Jump to latest", timeout: 5)
        try fill("Assistant message", "Return to the latest message")
        try press("Send")
        _ = try wait("Assistant is thinking", timeout: 5)
        try snapshot("assistant-thinking")
        try waitGone("Assistant is working", timeout: 5)
        try waitGone("Jump to latest", timeout: 5)
        try waitGone("Assistant is thinking", timeout: 5)
        try press("Toggle Assistant")
        try waitGone("Toggle conversation list")
        print("PASS: Assistant opt-in, docked chat, keyboard routing, direct SQL edit, and Undo")
    }
    /// Writes a new synthetic workspace with the UI scale and pane width at
    /// which the transcript cut off a wide table reply. At that width the
    /// table is slightly wider than an 80% reply, so a column wraps.
    func seedWideTableWorkspace() throws {
        let workspace = URL(fileURLWithPath: env["QROW_DATA_DIR"]!).appendingPathComponent("workspace.json")
        try require(!FileManager.default.fileExists(atPath: workspace.path), "The table check needs an empty workspace directory")
        let settings: [String: Any] = [
            "ui_scale": 1.1,
            "assistant": [
                "enabled": true,
                "data_sharing_notice_version": 1,
                "codex_executable": FileManager.default.currentDirectoryPath + "/tests/e2e/native/fake-codex.sh",
                "panel_width": 536,
            ],
        ]
        let data = try JSONSerialization.data(withJSONObject: [
            "version": 3, "settings": settings, "profiles": [], "tabs": [], "active_tab": 0,
        ])
        try data.write(to: workspace)
    }
    /// A reply with a wide table uses the transcript width, the transcript
    /// scrolls to the end of the table, and the wheel scrolls past the table.
    /// The check runs with the Connections sidebar shown and hidden.
    func testAssistantWideTable() throws {
        key(38, flags: .maskCommand) // Cmd+J opens the docked assistant.
        _ = try wait("Assistant model: Synthetic Model", timeout: 20)
        if find("Search conversations") != nil {
            try press("Toggle conversation list")
            try waitGone("Search conversations", timeout: 5)
        }
        // Earlier messages make the transcript long enough to scroll.
        try fill("Assistant message", "Show many lines")
        try press("Send")
        _ = try wait("Line 40", timeout: 20)
        try fill("Assistant message", "Show a wide table")
        try press("Send")
        _ = try wait("Assistant: Ran a synthetic wide table query", timeout: 20)
        try checkWideTableReply("assistant-wide-table")
        key(11, flags: .maskCommand) // Cmd+B hides the Connections sidebar.
        try waitGone("New Connection", timeout: 5)
        try checkWideTableReply("assistant-wide-table-no-sidebar")
        print("PASS: Assistant wide table replies use the transcript width, show the whole table, and scroll")
    }
    func checkWideTableReply(_ name: String) throws {
        // The tooltip has the same text, so look for the button only.
        func jumpButtonVisible() -> Bool { find("Jump to latest", role: kAXButtonRole) != nil }
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.5))
        if jumpButtonVisible() { try press("Jump to latest") }
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.5))
        try require(!jumpButtonVisible(), "The transcript did not scroll to the latest message")
        let reply = try wait("Assistant: Ran a synthetic wide table query", timeout: 5)
        let composer = try waitInput("Assistant message")
        let (origin, extent) = try elementBounds(reply)
        let (composerOrigin, composerSize) = try elementBounds(composer)
        try snapshot(name)

        // At the end of the transcript, the whole reply is above the composer.
        // A transcript that cannot scroll that far cuts off the table.
        try require(origin.y + extent.height <= composerOrigin.y, "The transcript cut off the bottom of the assistant table")
        try require(extent.width >= composerSize.width * 0.9, "The assistant reply did not use the full transcript width")

        // The wheel over the table scrolls the transcript in both directions.
        // A negative distance scrolls down at the same point.
        try scrollUpAbove(composer, by: 300)
        var deadline = Date(timeIntervalSinceNow: 5)
        while !jumpButtonVisible() {
            try require(Date() < deadline, "The wheel over the assistant table did not scroll the transcript up")
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
        }
        for _ in 0..<3 { try scrollUpAbove(composer, by: -1200) }
        deadline = Date(timeIntervalSinceNow: 5)
        while jumpButtonVisible() {
            try require(Date() < deadline, "The wheel over the assistant table did not scroll the transcript to the end")
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
        }
    }
    func testAssistantQueries() throws {
        try fill("SQL Editor", "SELECT 1 AS assistant_value")
        key(38, flags: .maskCommand) // Reopen the existing conversation.
        _ = try wait("Assistant query approval mode", timeout: 20)
        try fill("Assistant message", "Run selected SQL with approval")
        try press("Send")
        _ = try wait("Assistant query approval:", timeout: 20)
        let cancel = try waitExact("Cancel assistant turn", timeout: 5, role: kAXButtonRole)
        try require(find("Send", role: kAXButtonRole) == nil, "Send remained visible during the assistant turn")
        let (cancelPosition, _) = try elementBounds(cancel)
        let (composerPosition, composerSize) = try elementBounds(try waitInput("Assistant message"))
        try require(cancelPosition.y >= composerPosition.y + composerSize.height, "Cancel was not below the message field")
        let runButtons = elements().filter {
            attribute($0, kAXRoleAttribute) as? String == kAXButtonRole && strings($0).contains("Run")
        }
        try require(runButtons.count == 2, "Expected toolbar Run and assistant approval Run")
        try activate(runButtons[1])
        _ = try wait("I ran the query.", timeout: 90)
        _ = try wait("Send", timeout: 5, role: kAXButtonRole)
        _ = try wait("1", role: kAXCellRole)

        let mode = try wait("Assistant query approval mode", role: kAXPopUpButtonRole)
        try activate(mode)
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.3))
        key(125) // Down selects Run mode.
        key(36)
        try activate(try waitExact("Run automatically", timeout: 10, role: kAXButtonRole))
        _ = try wait("Run", timeout: 10, role: kAXPopUpButtonRole)
        try fill("SQL Editor", "SELECT 2 AS assistant_value")
        try fill("Assistant message", "Run selected SQL automatically")
        try press("Send")
        _ = try wait("2", role: kAXCellRole)
        try require(find("Assistant query approval:") == nil, "Automatic mode requested approval")
        try press("Toggle Assistant")
        try waitGone("Toggle conversation list")
        print("PASS: Assistant approval and automatic execution use the selected query tab")
    }
    func test() throws {
        try start()
        try testAbout()
        try testSettings()
        try testAssistant()
        try press("New Connection")
        for (label, value) in [("Name", "Qrow E2E"), ("Host", "127.0.0.1"),
                               ("Port", env["QROW_E2E_PORT"]!), ("Username", "qrow"),
                               ("Password", "qrow-test-password"), ("Initial database", "default")] {
            try fill(label, value)
        }
        try press("Save")
        _ = try wait("Qrow E2E")
        try waitGone("Cancel")

        // Connection validation uses one error alert and keeps the form open.
        try press("New Connection")
        try fill("Host", "127.0.0.1")
        try press("Save")
        _ = try wait("Enter a username.")
        _ = try wait("Username", role: kAXTextFieldRole)
        try press("Cancel")
        try waitGone("Cancel")

        // Creating another connection with the same name is rejected before
        // the password reaches Keychain.
        try press("New Connection")
        for (label, value) in [("Name", "Qrow E2E"), ("Host", "127.0.0.1"),
                               ("Port", env["QROW_E2E_PORT"]!), ("Username", "qrow"),
                               ("Password", "qrow-test-password"), ("Initial database", "default")] {
            try fill(label, value)
        }
        try press("Save")
        _ = try wait("A connection with this name already exists.")
        _ = try wait("Name", role: kAXTextFieldRole)
        try press("Cancel")
        try waitGone("Cancel")

        // Connection actions now live in the row context menu. Exercise each
        // action on the disposable fixture profile before opening its session.
        try contextMenu("Qrow E2E", exact: true, role: kAXButtonRole)
        _ = try wait("Edit Connection…")
        _ = try wait("Duplicate")
        _ = try wait("Delete")
        key(53)
        try waitGone("Edit Connection…")

        try contextMenu("Qrow E2E", exact: true, role: kAXButtonRole)
        try pressMenuItem("Edit Connection…")
        _ = try wait("Password", role: kAXTextFieldRole)
        try press("Cancel")
        try waitGone("Cancel")

        try contextMenu("Qrow E2E", exact: true, role: kAXButtonRole)
        try pressMenuItem("Duplicate")
        _ = try wait("Password", role: kAXTextFieldRole)
        try fill("Password", "qrow-test-password")
        try press("Save")
        try waitGone("Cancel")
        _ = try wait("Qrow E2E copy")

        // Renaming a connection cannot take another connection's name.
        try contextMenu("Qrow E2E copy", exact: true, role: kAXButtonRole)
        try pressMenuItem("Edit Connection…")
        _ = try wait("Password", role: kAXTextFieldRole)
        try fill("Name", "Qrow E2E")
        try press("Save")
        _ = try wait("A connection with this name already exists.")
        _ = try wait("Name", role: kAXTextFieldRole)
        try press("Cancel")
        try waitGone("Cancel")

        try contextMenu("Qrow E2E", exact: true, role: kAXButtonRole)
        try pressMenuItem("Duplicate")
        _ = try wait("Password", role: kAXTextFieldRole)
        try fill("Password", "qrow-test-password")
        try press("Save")
        try waitGone("Cancel")
        _ = try wait("Qrow E2E copy 2")
        try contextMenu("Qrow E2E copy 2", exact: true, role: kAXButtonRole)
        try pressMenuItem("Delete")
        try press("Delete connection")
        try waitGone("Qrow E2E copy 2")
        try contextMenu("Qrow E2E copy", role: kAXButtonRole)
        try pressMenuItem("Delete")
        // Alert titles are not exposed by GPUI's macOS accessibility tree.
        // The confirmation button proves that the alert replaced the menu.
        try press("Delete connection")
        try waitGone("Qrow E2E copy")

        try press("Qrow E2E")
        try testAssistantQueries()
        try query("SELECT 'qrow-ui-connected' AS result")
        _ = try wait("qrow-ui-connected", role: kAXCellRole)
        try testEditorHighlight()
        try snapshot("connected")

        // Create a second disposable profile for the connection-switch test.
        try contextMenu("Qrow E2E", exact: true, role: kAXButtonRole)
        try pressMenuItem("Duplicate")
        _ = try wait("Password", role: kAXTextFieldRole)
        try fill("Password", "qrow-test-password")
        try press("Save")
        try waitGone("Cancel")
        _ = try wait("Qrow E2E copy")

        // Creating a connection activates its default tab. Select A explicitly
        // before setting up its session; B keeps its default idle policy.
        try selectConnection("Qrow E2E")

        // Copy and move destination menus are nested under the tab context
        // menu. Keyboard navigation verifies that each submenu opens and its
        // first connection item performs the requested action.
        try contextMenu("Query 1", exact: true)
        _ = try wait("Copy to Connection…")
        for _ in 0..<3 { key(125) }
        // The context menu has room to open this submenu to the right in the
        // native test window.
        key(124)
        _ = try wait("Qrow E2E copy", role: kAXMenuItemRole)
        key(36)
        try waitGone("Copy to Connection…")
        try selectConnection("Qrow E2E copy")
        _ = try waitExact("Query 1 (Copy)", timeout: 10)
        _ = try wait("SELECT 'qrow-ui-connected' AS result", timeout: 10, role: kAXTextAreaRole)
        try require(
            find("qrow-ui-connected", role: kAXCellRole) == nil,
            "Copy carried results to the destination tab",
        )
        try selectConnection("Qrow E2E")
        _ = try wait("SELECT 'qrow-ui-connected' AS result", timeout: 10, role: kAXTextAreaRole)
        _ = try wait("qrow-ui-connected", timeout: 10, role: kAXCellRole)
        try press("Logs Panel")
        try require(
            !containsText("Selected connection", in: app),
            "Connection selection added an obsolete activity entry",
        )
        try press("Results Panel")
        try contextMenu("Query 1", exact: true)
        _ = try wait("Move to Connection…")
        for _ in 0..<4 { key(125) }
        key(124)
        _ = try wait("Qrow E2E copy", role: kAXMenuItemRole)
        key(36)
        try waitGone("Move to Connection…")
        try selectConnection("Qrow E2E copy")
        _ = try waitExact("Query 1 (Copy 2)", timeout: 10)
        _ = try wait("SELECT 'qrow-ui-connected' AS result", timeout: 10, role: kAXTextAreaRole)
        try require(
            find("qrow-ui-connected", role: kAXCellRole) == nil,
            "Move carried results to the destination tab",
        )
        try selectConnection("Qrow E2E")
        try waitGone("Query 1 (Copy 2)")
        _ = try waitExact("Query 1", timeout: 10)
        try require(
            find("SELECT 'qrow-ui-connected' AS result", role: kAXTextAreaRole) == nil,
            "Move left the source SQL on its original connection",
        )
        try require(
            find("qrow-ui-connected", role: kAXCellRole) == nil,
            "Move left source rows in its replacement tab",
        )

        let keepAliveToken = "keep-alive-" + UUID().uuidString.lowercased()
        try contextMenu("Qrow E2E", exact: true, role: kAXButtonRole)
        try pressMenuItem("Edit Connection…")
        try selectPopup("When idle", "Keep connected")
        try scrollDown(try wait("Name", role: kAXTextFieldRole))
        try fill("Keep-alive interval in seconds", "3")
        try fill("Keep-alive query", "SELECT qrow_keep_alive(id, '\(keepAliveToken)', CAST(30000 AS BIGINT)) FROM range(1)")
        try press("Save")
        try waitGone("Cancel")
        try query("CREATE TEMPORARY FUNCTION qrow_keep_alive AS 'io.qrow.fixture.Blocking'")
        // The first keep-alive can replace the short-lived Complete status
        // before Accessibility observes it. Sending keep-alive proves the
        // setup statement finished and the session entered its idle policy.
        _ = try waitAny(["Complete", "Sending keep-alive…"])
        try query("SET spark.sql.session.timeZone=Asia/Tokyo")
        _ = try wait("Asia/Tokyo", role: kAXCellRole)

        // Selection preserves the original cursor as well as downloaded rows.
        try query("SELECT concat('switch-a-', lpad(CAST(id AS STRING), 4, '0')) AS value FROM range(2001) ORDER BY id")
        _ = try wait("switch-a-0000")
        try press("Next")
        _ = try wait("switch-a-1000")
        _ = try wait("Page 2")
        // Rows arrive before Ready while the worker is still fetching the page.
        // Wait for the completed preview before expecting idle maintenance.
        _ = try wait("Preview · More rows available")
        _ = try wait("Sending keep-alive…", timeout: 20)
        try selectConnection("Qrow E2E copy")
        // A's tabs are hidden while B is selected, so use the connection row
        // indicator to verify that A's heartbeat is still running.
        _ = try wait("Qrow E2E, running", timeout: 5)
        try contextMenu("Qrow E2E, running", role: kAXButtonRole)
        try pressMenuItem("Edit Connection…")
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.3))
        try require(find("Password", role: kAXTextFieldRole) == nil, "Edit opened during the original session's heartbeat")
        key(53)
        try waitGone("Edit Connection…")
        try selectConnection("Qrow E2E")
        _ = try wait("switch-a-1000")
        _ = try wait("Connected · Keep-alive enabled", timeout: 40)
        _ = try wait("switch-a-1000")
        try press("Next")
        _ = try wait("switch-a-2000")
        _ = try wait("Page 3")
        try snapshot("connection-switch-keep-alive")
        try press("Previous")
        _ = try wait("switch-a-1000")
        try query("SELECT concat('same-session-', current_timezone()) AS value")
        _ = try wait("same-session-Asia/Tokyo")

        // Run on B changes the live session and replaces A's preview. B uses
        // the server's default time zone, not A's session setting.
        try selectConnection("Qrow E2E copy")
        try query("SELECT concat('switch-b-', lpad(CAST(id AS STRING), 4, '0'), '-', current_timezone()) AS value FROM range(4001) ORDER BY id")
        _ = try wait("switch-b-0000-UTC")
        try selectConnection("Qrow E2E")
        _ = try wait("same-session-Asia/Tokyo")
        // Disconnect acts on the active tab only. A's session closes while
        // B's result cursor remains available in its hidden tab.
        _ = try wait("Connected · Keep-alive enabled", timeout: 40)
        try waitGone("Sending keep-alive…", timeout: 40)
        try press("Disconnect")
        _ = try wait("Disconnected", timeout: 10)
        try selectConnection("Qrow E2E copy")
        _ = try wait("switch-b-0000-UTC")
        _ = try wait("Preview · More rows available")
        try press("Next")
        _ = try wait("switch-b-1000-UTC")
        try snapshot("connection-switch")

        // Editing A while B is selected must not close B's session. Restore A's
        // default idle policy, then fetch through B's cursor.
        try contextMenu("Qrow E2E", exact: true, role: kAXButtonRole)
        try pressMenuItem("Edit Connection…")
        try selectPopup("When idle", "Disconnect after")
        try press("Save")
        try waitGone("Cancel")
        try selectConnection("Qrow E2E copy")
        try press("Next")
        _ = try wait("switch-b-2000-UTC")

        // A lifecycle edit updates B's live session while A remains selected.
        try selectConnection("Qrow E2E")
        try contextMenu("Qrow E2E copy", role: kAXButtonRole)
        try pressMenuItem("Edit Connection…")
        try selectPopup("When idle", "Keep connected")
        try scrollDown(try wait("Name", role: kAXTextFieldRole))
        try fill("Keep-alive interval in seconds", "3")
        try fill("Keep-alive query", "SELECT 'updated-b'")
        try press("Save")
        try waitGone("Cancel")
        try selectConnection("Qrow E2E copy")
        _ = try wait("Connected · Keep-alive enabled", timeout: 20)
        _ = try wait("switch-b-2000-UTC")
        try press("Next")
        _ = try wait("switch-b-3000-UTC")

        // Replacing B's password closes B even while A is selected. Restore
        // B's default idle policy for the remaining disconnect scenarios.
        try selectConnection("Qrow E2E")
        try contextMenu("Qrow E2E copy", role: kAXButtonRole)
        try pressMenuItem("Edit Connection…")
        try fill("Password", "qrow-test-password")
        try selectPopup("When idle", "Disconnect after")
        try press("Save")
        try waitGone("Cancel")
        try selectConnection("Qrow E2E copy")
        _ = try wait("Not connected")
        _ = try wait("switch-b-3000-UTC")
        try query("SELECT 'switch-b-reconnected' AS value")
        _ = try wait("switch-b-reconnected")
        try selectConnection("Qrow E2E")
        // Returning to the session's profile enables Disconnect without a Run.
        try selectConnection("Qrow E2E copy")
        try press("Disconnect")
        _ = try wait("Disconnected", timeout: 10)
        _ = try wait("switch-b-reconnected")
        try query("SELECT 'switch-b-after-disconnect' AS value")
        _ = try wait("switch-b-after-disconnect")
        try selectConnection("Qrow E2E")
        try contextMenu("Qrow E2E copy", exact: true, role: kAXButtonRole)
        try pressMenuItem("Delete")
        try press("Delete connection")
        try waitGone("Qrow E2E copy")
        _ = try wait("Disconnected")

        // A profile metadata edit keeps the session in both tabs. Temporary
        // views prove that the workers did not reconnect when the form was saved.
        try query("CREATE TEMPORARY VIEW qrow_ui_live AS SELECT 'preserved' AS value")
        _ = try wait("Complete")
        try newTab("Query 2")
        try press("Qrow E2E")
        try query("CREATE TEMPORARY VIEW qrow_ui_live AS SELECT 'preserved' AS value")
        _ = try wait("Complete")
        try click(try waitExact("Query 1"))
        try contextMenu("Qrow E2E", exact: true, role: kAXButtonRole)
        try pressMenuItem("Edit Connection…")
        try fill("Name", "Qrow E2E live")
        try press("Save")
        try waitGone("Cancel")
        _ = try wait("Qrow E2E live")
        try query("SELECT * FROM qrow_ui_live")
        _ = try wait("preserved")
        try click(try waitExact("Query 2"))
        // Wait for the new tab's editor before fill captures its accessibility node.
        _ = try wait("CREATE TEMPORARY VIEW qrow_ui_live AS SELECT 'preserved' AS value", role: kAXTextAreaRole)
        try query("SELECT * FROM qrow_ui_live")
        _ = try wait("preserved")
        try click(try waitExact("Query 1"))

        // Duplicate keeps tab names unique within the connection and selects
        // the new tab. A second copy receives a numbered suffix.
        try contextMenu("Query 1", exact: true)
        try pressMenuItem("Duplicate")
        _ = try waitExact("Query 1 (Copy)")
        try click(try waitExact("Query 1"))
        try contextMenu("Query 1", exact: true)
        try pressMenuItem("Duplicate")
        _ = try waitExact("Query 1 (Copy 2)")
        try click(try waitExact("Query 1"))

        // A rename cannot take another tab's name on this connection. The
        // dialog stays open and the tab title stays unchanged. GPUI does not
        // publish the validation text in the macOS accessibility tree.
        try contextMenu("Query 1", exact: true)
        try pressMenuItem("Rename…")
        let tabName = try wait("Tab Name", role: kAXTextFieldRole)
        try require(
            attribute(tabName, kAXValueAttribute) as? String == "Query 1",
            "Rename form did not prefill the current tab name"
        )
        try fill("Tab Name", "Query 2")
        try press("Rename")
        _ = try wait("Tab Name", role: kAXTextFieldRole)
        _ = try waitExact("Query 1")
        try press("Cancel")
        try waitGone("Tab Name")

        // The tab menu renames the tab without changing its SQL or session.
        try contextMenu("Query 1", exact: true)
        try pressMenuItem("Rename…")
        _ = try wait("Tab Name", role: kAXTextFieldRole)
        try fill("Tab Name", String(repeating: "x", count: 61))
        try press("Rename")
        // Validation text is not exposed by GPUI's accessibility tree. A
        // rejected rename leaves the dialog open and the tab title unchanged.
        _ = try wait("Tab Name", role: kAXTextFieldRole)
        _ = try waitExact("Query 1")
        // A later duplicate-name validation also leaves the dialog open.
        try fill("Tab Name", "Query 2")
        try press("Rename")
        _ = try wait("Tab Name", role: kAXTextFieldRole)
        try press("Cancel")
        try waitGone("Tab Name")
        try contextMenu("Query 1", exact: true)
        try pressMenuItem("Rename…")
        _ = try wait("Tab Name", role: kAXTextFieldRole)
        try fill("Tab Name", "Renamed tab")
        try press("Rename")
        try waitGone("Tab Name")
        _ = try waitExact("Renamed tab")
        try contextMenu("Renamed tab", exact: true)
        try pressMenuItem("Rename…")
        _ = try wait("Tab Name", role: kAXTextFieldRole)
        try press("Rename")
        try waitGone("Tab Name")
        _ = try waitExact("Renamed tab")

        try query("SELECT concat('row-', lpad(CAST(id AS STRING), 4, '0')) AS value FROM range(1001) ORDER BY id")
        _ = try wait("row-0000")
        _ = try wait("Preview · More rows available")
        try press("Next")
        _ = try wait("row-1000")
        _ = try wait("Page 2")
        try press("Previous")
        _ = try wait("row-0000")
        _ = try wait("Page 1")
        try snapshot("pagination")

        // A bad prefix proves only the selected statement reaches Spark. Selection is UTF-16.
        try fill("SQL Editor", "invalid prefix;\nSELECT '日本語😀' AS selected_value")
        key(123, flags: [.maskCommand, .maskShift])
        key(36, flags: .maskCommand)
        _ = try wait("日本語😀")
        try snapshot("unicode-selection")

        try query("CREATE TEMPORARY FUNCTION qrow_block AS 'io.qrow.fixture.Blocking'")
        try waitGone("日本語😀")
        _ = try wait("Complete")

        // Issue #40: a retry must clear the previous Error badge before the
        // replacement query finishes, even after the user selected Results.
        try query("SELECT missing_column AS value FROM range(1)")
        _ = try wait("Error · Query failed", timeout: 30)
        _ = try waitExact("Renamed tab, unread error", timeout: 30)
        try press("Results Panel")
        try require(
            findExact("Renamed tab, unread error") != nil,
            "Selecting Results acknowledged an unread error",
        )
        let retryToken = "badge-" + UUID().uuidString.lowercased()
        try query("SELECT qrow_block(id, '\(retryToken)', CAST(3000 AS BIGINT)) AS value FROM range(1)")
        var deadline = clock.now.advanced(by: .seconds(30))
        while try command(["python3", "scripts/e2e/run.py", "observe", "count", "\(retryToken).started"]) != "1" {
            try require(clock.now < deadline, "Spark executor never started the badge retry")
            Thread.sleep(forTimeInterval: 0.1)
        }
        _ = try waitExact("Renamed tab, running", timeout: 20)
        try require(
            findExact("Renamed tab, running, unread error") == nil,
            "Retry still displayed the previous Error badge while running",
        )
        try press("Logs Panel")
        _ = try waitExact("Renamed tab", timeout: 30)
        try require(
            findExact("Renamed tab, unread error") == nil,
            "Successful retry kept the Error badge",
        )
        try snapshot("error-badge-cleared")

        // Repeat the basic reproduction without an intervening panel click.
        try query("SELECT missing_column AS value FROM range(1)")
        _ = try waitExact("Renamed tab, unread error", timeout: 30)
        let secondRetryToken = "badge-" + UUID().uuidString.lowercased()
        try query("SELECT qrow_block(id, '\(secondRetryToken)', CAST(1000 AS BIGINT)) AS value FROM range(1)")
        deadline = clock.now.advanced(by: .seconds(30))
        while try command(["python3", "scripts/e2e/run.py", "observe", "count", "\(secondRetryToken).started"]) != "1" {
            try require(clock.now < deadline, "Spark executor never started the second badge retry")
            Thread.sleep(forTimeInterval: 0.1)
        }
        _ = try waitExact("Renamed tab, running", timeout: 20)
        try require(
            findExact("Renamed tab, running, unread error") == nil,
            "Second retry still displayed the previous Error badge while running",
        )
        _ = try waitExact("Renamed tab", timeout: 30)
        try require(
            findExact("Renamed tab, unread error") == nil,
            "Second successful retry kept the Error badge",
        )

        let token = "ui-" + UUID().uuidString.lowercased()
        try query("SELECT qrow_block(id, '\(token)', CAST(60000 AS BIGINT)) FROM range(1)")
        deadline = clock.now.advanced(by: .seconds(150))
        while try command(["python3", "scripts/e2e/run.py", "observe", "count", "\(token).started"]) != "1" {
            try require(clock.now < deadline, "Spark executor never started UI query")
            Thread.sleep(forTimeInterval: 0.1)
        }
        // The evidence file can be written before the UI replaces the prior
        // result status. Synchronize on the running tab before checking that
        // busy connection actions remain unavailable.
        _ = try waitExact("Renamed tab, running", timeout: 20)
        try contextMenu("Qrow E2E live", role: kAXButtonRole)
        // GPUI exposes these as disabled menu items visually, but does not
        // publish AXEnabled on macOS. Verify their observable no-op behavior.
        try pressMenuItem("Edit Connection…")
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.3))
        try require(find("Password", role: kAXTextFieldRole) == nil, "Edit opened while the connection was busy")
        try pressMenuItem("Delete")
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.3))
        try require(find("Delete connection", role: kAXButtonRole) == nil, "Delete confirmation opened while the connection was busy")
        key(53)
        try waitGone("Edit Connection…")
        try newTab("Query 3")
        try press("Qrow E2E live")
        try query("SELECT 'other-tab-works' AS result")
        _ = try wait("other-tab-works")
        try click(try wait("Renamed tab, running"))
        let cancelStarted = clock.now
        try press("Cancel")
        deadline = cancelStarted.advanced(by: .seconds(10))
        while try command(["python3", "scripts/e2e/run.py", "observe", "count", "\(token).interrupted"]) != "1" {
            try require(clock.now < deadline, "UI cancellation did not stop Spark within 10 seconds")
            Thread.sleep(forTimeInterval: 0.1)
        }
        while try command(["python3", "scripts/e2e/run.py", "observe", "count", "\(token).ended"]) != "1" {
            try require(clock.now < deadline, "Spark driver did not confirm terminal task within 10 seconds")
            Thread.sleep(forTimeInterval: 0.1)
        }
        _ = try wait("Cancelled · Partial preview retained", timeout: 2)
        try require(clock.now <= deadline, "UI cancellation exceeded 10 seconds")
        try require(try command(["python3", "scripts/e2e/run.py", "observe", "count", "\(token).completed"]) == "0", "Cancelled query completed")
        try snapshot("cancelled")
        try query("SELECT 'after-cancel-works' AS result")
        _ = try wait("after-cancel-works")
        try press("Disconnect")
        try query("SELECT 'reconnect-works' AS result")
        _ = try wait("reconnect-works")
        try snapshot("reconnected")
        try testFailedSaveExit(closeWindow: false)
        print("PASS: About dialog, Settings dialog, connection menus, connection validation, unique connection names, tab duplication, unique tab names, tab rename, connection form, connection switching, retained results, real results, pagination, Unicode selection, concurrent tabs, server cancellation, reconnect")
    }
}

do {
    try require(AXIsProcessTrusted(), "Native UI tests require Accessibility permission for the driver/terminal. No UI tests ran.")
    try require(CGPreflightScreenCaptureAccess(), "Native UI tests require Screen Recording permission for failure screenshots. No UI tests ran.")
    if !CommandLine.arguments.contains("--preflight") {
        let driver = Driver()
        do {
            if CommandLine.arguments.contains("--persistence-only") {
                try driver.start()
                try driver.testFailedSaveExit(closeWindow: false)
            } else if CommandLine.arguments.contains("--dialogs-only") {
                // The About and Settings dialogs need no server, so they can run alone.
                try driver.start()
                try driver.testAbout()
                try driver.testSettings()
            } else if CommandLine.arguments.contains("--assistant-only") {
                try driver.start()
                try driver.testAssistant()
            } else if CommandLine.arguments.contains("--assistant-font-only") {
                try driver.start()
                try driver.testAssistant(fontOnly: true)
            } else if CommandLine.arguments.contains("--assistant-table-only") {
                try driver.seedWideTableWorkspace()
                try driver.start()
                try driver.testAssistantWideTable()
            } else if CommandLine.arguments.contains("--editor-highlight-only") {
                try driver.start()
                try driver.testEditorHighlight()
            } else {
                try driver.test()
            }
            driver.stop()
        }
        catch { if driver.app != nil { try? driver.snapshot("failure") }; driver.stop(); throw error }
        if CommandLine.arguments.contains("--assistant-font-only") {
            let restartDriver = Driver(name: "qrow-assistant-restart")
            do {
                try restartDriver.start()
                try restartDriver.testAssistantRestart()
                restartDriver.stop()
            } catch {
                if restartDriver.app != nil { try? restartDriver.snapshot("failure-assistant-restart") }
                restartDriver.stop()
                throw error
            }
        }
        if CommandLine.arguments.contains("--editor-highlight-only")
            || CommandLine.arguments.contains("--assistant-table-only") { exit(0) }
        let windowDriver = Driver(name: "qrow-window-close")
        do {
            try windowDriver.start()
            try windowDriver.testFailedSaveExit(closeWindow: true)
            windowDriver.stop()
        } catch {
            if windowDriver.app != nil { try? windowDriver.snapshot("failure-window-close") }
            windowDriver.stop()
            throw error
        }
    }
} catch {
    fputs("\(error)\n", stderr)
    exit(1)
}
