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
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.3))
    guard let position = attribute(element, kAXPositionAttribute),
          let size = attribute(element, kAXSizeAttribute) else { throw Failure("Element has no bounds") }
    var point = CGPoint.zero
    var extent = CGSize.zero
    try require(CFGetTypeID(position) == AXValueGetTypeID() && CFGetTypeID(size) == AXValueGetTypeID(), "Invalid element bounds")
    AXValueGetValue(unsafeBitCast(position, to: AXValue.self), .cgPoint, &point)
    AXValueGetValue(unsafeBitCast(size, to: AXValue.self), .cgSize, &extent)
    point.x += extent.width / 2
    point.y += extent.height / 2
    let move = CGEvent(mouseEventSource: nil, mouseType: .mouseMoved, mouseCursorPosition: point, mouseButton: .left)!
    move.post(tap: .cghidEventTap)
    let scroll = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: -1200, wheel2: 0, wheel3: 0)!
    scroll.location = point
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
            (role == nil || attribute($0, kAXRoleAttribute) as? String == role) && strings($0).contains(label)
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
    func press(_ label: String) throws {
        let deadline = clock.now.advanced(by: .seconds(150))
        repeat {
            let control = find(label, role: kAXButtonRole) ?? find(label, role: kAXCheckBoxRole)
            if let control, attribute(control, kAXEnabledAttribute) as? Bool != false {
                try click(control)
                return
            }
            try require(process.isRunning, "Qrow exited while waiting for button: \(label)")
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
        } while clock.now < deadline
        throw Failure("Button never became enabled: \(label)")
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
    func fill(_ label: String, _ value: String) throws {
        // GPUI exposes inputs as text fields or text areas depending on the control.
        let deadline = clock.now.advanced(by: .seconds(10))
        var input: AXUIElement?
        repeat {
            input = elements().first {
                [kAXTextFieldRole, kAXTextAreaRole].contains(attribute($0, kAXRoleAttribute) as? String ?? "") && strings($0).contains(label)
            }
            if input != nil { break }
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.05))
        } while clock.now < deadline
        guard let element = input else { throw Failure("Missing accessible input: \(label)") }
        try click(element)
        // Let the native click establish editor focus before sending keyboard shortcuts.
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.2))
        key(0, flags: .maskCommand)
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
        if label == "Password" {
            // macOS intentionally withholds secure text values.
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
        } else {
            let deadline = clock.now.advanced(by: .seconds(5))
            while attribute(element, kAXValueAttribute) as? String != value {
                try require(clock.now < deadline, "Input did not accept text: \(label)")
                RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.05))
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
        process.environment = env
        process.standardOutput = log
        process.standardError = log
        let started = clock.now
        try process.run()
        inputPID = process.processIdentifier
        app = AXUIElementCreateApplication(process.processIdentifier)
        NSRunningApplication(processIdentifier: process.processIdentifier)?.activate(options: [])
        _ = try wait("New Connection", timeout: 20)
        samples.append("launch_to_accessible_new_connection_seconds=\(started.duration(to: clock.now))")
        sampleTimer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
            guard let self else { return }
            if let sample = try? command(["ps", "-o", "rss=,%cpu=", "-p", "\(self.process.processIdentifier)"]) {
                self.samples.append("\(Date().timeIntervalSince1970) \(sample)")
            }
        }
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
    func testSettings() throws {
        try selectApplicationMenuItem("Settings…")
        // Every setting stays on one page, so each control is reachable without
        // the pointer-only section list.
        try waitSettingValue("UI Scale", "100")
        try waitSettingValue("Editor Font Size", "13")
        try waitSettingValue("Logs Font Size", "13")
        try press("Increase Editor Font Size")
        try waitSettingValue("Editor Font Size", "14")
        try snapshot("settings")
        try press("Restore defaults")
        try waitSettingValue("Editor Font Size", "13")
        try press("Save")
        try waitGone("UI Scale")
        print("PASS: Settings shows every control, applies a change, and restores defaults")
    }
    func test() throws {
        try start()
        try testAbout()
        try testSettings()
        try press("New Connection")
        for (label, value) in [("Name", "Qrow E2E"), ("Host", "127.0.0.1"),
                               ("Port", env["QROW_E2E_PORT"]!), ("Username", "qrow"),
                               ("Password", "qrow-test-password"), ("Initial Database", "default")] {
            try fill(label, value)
        }
        try press("Save")
        _ = try wait("Qrow E2E")
        try waitGone("Cancel")

        // Connection actions now live in the row context menu. Exercise each
        // action on the disposable fixture profile before opening its session.
        try rightClick(try waitExact("Qrow E2E", role: kAXButtonRole))
        _ = try wait("Edit Connection…")
        _ = try wait("Duplicate")
        _ = try wait("Delete")
        key(53)
        try waitGone("Edit Connection…")

        try rightClick(try waitExact("Qrow E2E", role: kAXButtonRole))
        try click(try wait("Edit Connection…"))
        _ = try wait("Password", role: kAXTextFieldRole)
        try press("Cancel")
        try waitGone("Cancel")

        try rightClick(try waitExact("Qrow E2E", role: kAXButtonRole))
        try click(try wait("Duplicate"))
        _ = try wait("Password", role: kAXTextFieldRole)
        try fill("Password", "qrow-test-password")
        try press("Save")
        try waitGone("Cancel")
        _ = try wait("Qrow E2E copy")
        try rightClick(try waitExact("Qrow E2E", role: kAXButtonRole))
        try click(try wait("Duplicate"))
        _ = try wait("Password", role: kAXTextFieldRole)
        try fill("Password", "qrow-test-password")
        try press("Save")
        try waitGone("Cancel")
        _ = try wait("Qrow E2E copy 2")
        try rightClick(try waitExact("Qrow E2E copy 2", role: kAXButtonRole))
        try click(try wait("Delete"))
        try press("Delete connection")
        try waitGone("Qrow E2E copy 2")
        try rightClick(try wait("Qrow E2E copy", role: kAXButtonRole))
        try click(try wait("Delete"))
        // Alert titles are not exposed by GPUI's macOS accessibility tree.
        // The confirmation button proves that the alert replaced the menu.
        try press("Delete connection")
        try waitGone("Qrow E2E copy")

        try press("Qrow E2E")
        try query("SELECT 'qrow-ui-connected' AS result")
        _ = try wait("qrow-ui-connected", role: kAXCellRole)
        try snapshot("connected")

        // Create a second disposable profile for the connection-switch test.
        try rightClick(try waitExact("Qrow E2E", role: kAXButtonRole))
        try click(try wait("Duplicate"))
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
        try rightClick(try waitExact("Query 1"))
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
        try rightClick(try waitExact("Query 1"))
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
        try rightClick(try waitExact("Qrow E2E", role: kAXButtonRole))
        try click(try wait("Edit Connection…"))
        try scrollDown(try wait("Name", role: kAXTextFieldRole))
        try click(try wait("Keep Connected", role: kAXRadioButtonRole))
        try scrollDown(try wait("Keep Connected", role: kAXRadioButtonRole))
        try fill("Keep-alive Interval in Seconds", "3")
        try fill("Keep-alive Query", "SELECT qrow_keep_alive(id, '\(keepAliveToken)', CAST(30000 AS BIGINT)) FROM range(1)")
        try press("Save")
        try waitGone("Cancel")
        try query("CREATE TEMPORARY FUNCTION qrow_keep_alive AS 'io.qrow.fixture.Blocking'")
        _ = try wait("Complete")
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
        try rightClick(try wait("Qrow E2E, running", role: kAXButtonRole))
        try click(try wait("Edit Connection…"))
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
        try query("SELECT concat('switch-b-', lpad(CAST(id AS STRING), 4, '0'), '-', current_timezone()) AS value FROM range(2001) ORDER BY id")
        _ = try wait("switch-b-0000-UTC")
        try selectConnection("Qrow E2E")
        _ = try wait("same-session-Asia/Tokyo")
        // Disconnect acts on the active tab only. A's session closes while
        // B's result cursor remains available in its hidden tab.
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
        try rightClick(try waitExact("Qrow E2E", role: kAXButtonRole))
        try click(try wait("Edit Connection…"))
        try scrollDown(try wait("Name", role: kAXTextFieldRole))
        try click(try wait("Disconnect", role: kAXRadioButtonRole))
        try press("Save")
        try waitGone("Cancel")
        try selectConnection("Qrow E2E copy")
        try press("Next")
        _ = try wait("switch-b-2000-UTC")

        // A lifecycle edit updates B's live session while A remains selected.
        try selectConnection("Qrow E2E")
        try rightClick(try wait("Qrow E2E copy", role: kAXButtonRole))
        try click(try wait("Edit Connection…"))
        try scrollDown(try wait("Name", role: kAXTextFieldRole))
        try click(try wait("Keep Connected", role: kAXRadioButtonRole))
        try scrollDown(try wait("Keep Connected", role: kAXRadioButtonRole))
        try fill("Keep-alive Interval in Seconds", "3")
        try fill("Keep-alive Query", "SELECT 'updated-b'")
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
        try rightClick(try wait("Qrow E2E copy", role: kAXButtonRole))
        try click(try wait("Edit Connection…"))
        try fill("Password", "qrow-test-password")
        try scrollDown(try wait("Name", role: kAXTextFieldRole))
        try click(try wait("Disconnect", role: kAXRadioButtonRole))
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
        try rightClick(try waitExact("Qrow E2E copy", role: kAXButtonRole))
        try click(try wait("Delete"))
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
        try rightClick(try waitExact("Qrow E2E", role: kAXButtonRole))
        try click(try wait("Edit Connection…"))
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
        try rightClick(try waitExact("Query 1"))
        try click(try wait("Duplicate"))
        _ = try waitExact("Query 1 (Copy)")
        try click(try waitExact("Query 1"))
        try rightClick(try waitExact("Query 1"))
        try click(try wait("Duplicate"))
        _ = try waitExact("Query 1 (Copy 2)")
        try click(try waitExact("Query 1"))

        // A rename cannot take another tab's name on this connection. The
        // error alert explains why, while the dialog stays open for correction.
        try rightClick(try waitExact("Query 1"))
        try click(try wait("Rename…"))
        _ = try wait("Tab Name", role: kAXTextFieldRole)
        try fill("Tab Name", "Query 2")
        try press("Rename")
        _ = try wait("A tab with this name already exists on this connection.")
        _ = try wait("Tab Name", role: kAXTextFieldRole)
        _ = try waitExact("Query 1")
        try press("Cancel")
        try waitGone("Tab Name")

        // The tab menu renames the tab without changing its SQL or session.
        try rightClick(try waitExact("Query 1"))
        try click(try wait("Rename…"))
        _ = try wait("Tab Name", role: kAXTextFieldRole)
        try fill("Tab Name", String(repeating: "x", count: 61))
        try press("Rename")
        // Validation text is not exposed by GPUI's accessibility tree. A
        // rejected rename leaves the dialog open and the tab title unchanged.
        _ = try wait("Tab Name", role: kAXTextFieldRole)
        _ = try waitExact("Query 1")
        // A later duplicate-name error replaces the earlier length error.
        try fill("Tab Name", "Query 2")
        try press("Rename")
        _ = try wait("A tab with this name already exists on this connection.")
        _ = try wait("Tab Name", role: kAXTextFieldRole)
        try press("Cancel")
        try waitGone("Tab Name")
        try rightClick(try waitExact("Query 1"))
        try click(try wait("Rename…"))
        _ = try wait("Tab Name", role: kAXTextFieldRole)
        try fill("Tab Name", "Renamed tab")
        try press("Rename")
        try waitGone("Tab Name")
        _ = try waitExact("Renamed tab")
        try rightClick(try waitExact("Renamed tab"))
        try click(try wait("Rename…"))
        _ = try wait("Tab Name", role: kAXTextFieldRole)
        try press("Rename")
        try waitGone("Tab Name")
        _ = try waitExact("Renamed tab")

        try query("SELECT concat('row-', lpad(CAST(id AS STRING), 4, '0')) AS value FROM range(1001) ORDER BY id")
        _ = try wait("row-0000")
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
        try rightClick(try waitExact("Qrow E2E live", role: kAXButtonRole))
        // GPUI exposes these as disabled menu items visually, but does not
        // publish AXEnabled on macOS. Verify their observable no-op behavior.
        try click(try wait("Edit Connection…"))
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.3))
        try require(find("Password", role: kAXTextFieldRole) == nil, "Edit opened while the connection was busy")
        try click(try wait("Delete"))
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
        print("PASS: About dialog, Settings dialog, connection menus, tab duplication, unique tab names, tab rename, connection form, connection switching, retained results, real results, pagination, Unicode selection, concurrent tabs, server cancellation, reconnect")
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
            } else {
                try driver.test()
            }
            driver.stop()
        }
        catch { if driver.app != nil { try? driver.snapshot("failure") }; driver.stop(); throw error }
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
