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
/// Returns the point to click for an element. Query tabs scroll under the fixed
/// Toggle Sidebar and New Tab controls of the tab strip. The middle of a partly
/// hidden tab can then be over one of these controls, so for a tab the point is
/// the middle of its visible part.
func clickTarget(_ element: AXUIElement, _ point: CGPoint, _ extent: CGSize) -> CGPoint {
    var target = CGPoint(x: point.x + extent.width / 2, y: point.y + extent.height / 2)
    guard attribute(element, kAXRoleAttribute) as? String == kAXRadioButtonRole,
          let window = attribute(element, kAXWindowAttribute) else { return target }
    var left = point.x
    var right = point.x + extent.width
    for control in descendants(unsafeBitCast(window, to: AXUIElement.self)) {
        guard attribute(control, kAXRoleAttribute) as? String == kAXButtonRole,
              let label = strings(control).first, ["Toggle Sidebar", "New Tab"].contains(label),
              let (controlPoint, controlExtent) = try? elementBounds(control),
              abs(controlPoint.y + controlExtent.height / 2 - target.y) < extent.height / 2 else { continue }
        if label == "Toggle Sidebar" {
            left = max(left, controlPoint.x + controlExtent.width)
        } else {
            right = min(right, controlPoint.x)
        }
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
func scrollLogsToTop(_ app: AXUIElement) throws {
    guard let window = (attribute(app, kAXWindowsAttribute) as? [AXUIElement])?.first else {
        throw Failure("Qrow window is unavailable while scrolling Logs")
    }
    let (origin, size) = try elementBounds(window)
    let point = CGPoint(x: origin.x + size.width * 0.75, y: origin.y + size.height * 0.82)
    let move = CGEvent(mouseEventSource: nil, mouseType: .mouseMoved, mouseCursorPosition: point, mouseButton: .left)!
    move.post(tap: .cghidEventTap)
    for _ in 0..<32 {
        let scroll = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: 1200, wheel2: 0, wheel3: 0)!
        scroll.location = point
        scroll.post(tap: .cghidEventTap)
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.02))
    }
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
    /// Moves keyboard focus into the open submenu of `parent`. A submenu that
    /// does not fit on the right opens on the left. Then Left enters it, as in
    /// native macOS menus, and Right closes it.
    func enterSubmenu(_ parent: String, showing item: String) throws {
        let child = try wait(item, role: kAXMenuItemRole)
        let (childPosition, _) = try elementBounds(child)
        let (parentPosition, _) = try elementBounds(try wait(parent))
        key(childPosition.x < parentPosition.x ? 123 : 124)
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.2))
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
    /// Waits for the status of a completed query. Rows appear before the
    /// query completes, and completion selects the Results panel.
    func waitQueryComplete(timeout: Double = 30) throws {
        let deadline = clock.now.advanced(by: .seconds(timeout))
        while !elements().contains(where: { strings($0).contains { $0.hasPrefix("Complete") } }) {
            try require(clock.now < deadline, "Query did not complete")
            try require(process.isRunning, "Qrow exited while waiting for the query to complete")
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
        }
    }
    func testActivityRetention() throws {
        let started = clock.now
        try press("Logs Panel")
        try press("Clear Logs History")
        // Logs keep 100 activity groups (MAX_EXECUTION_GROUPS in
        // src/activity.rs). A rejected statement makes a group without a
        // server request, so only the first and the last query go to Spark.
        try query("SELECT 'retention-oldest' AS value")
        _ = try wait("retention-oldest", timeout: 30, role: kAXCellRole)
        try waitQueryComplete()
        try fill("SQL Editor", "SELECT 'retention-rejected'; SELECT 2")
        let run = try wait("Run", role: kAXButtonRole)
        for _ in 0..<100 {
            try require(AXUIElementPerformAction(run, kAXPressAction as CFString) == .success, "Run did not accept a press")
        }
        try query("SELECT 'retention-latest' AS value")
        _ = try wait("retention-latest", timeout: 30, role: kAXCellRole)
        try waitQueryComplete()
        try press("Logs Panel")
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
        try press("Copy All Logs")
        let deadline = clock.now.advanced(by: .seconds(5))
        var copied = clipboard.string(forType: .string) ?? ""
        while (!copied.hasPrefix("Older activity was removed\n") || !copied.contains("retention-latest"))
            && clock.now < deadline {
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.05))
            copied = clipboard.string(forType: .string) ?? ""
        }
        try require(
            copied.hasPrefix("Older activity was removed\n") && copied.contains("retention-latest")
                && copied.contains("Run one statement at a time"),
            "Copy All did not put the retention boundary before the retained entries: \(copied.prefix(120))"
        )
        try require(!copied.contains("retention-oldest"), "Logs retained the oldest query after the limit")
        try require(
            !copied.contains("reconnect-works"),
            "Logs retained activity from before the retention scenario"
        )
        // Copy All focuses its toolbar button; reset the viewport after that
        // interaction so the screenshot shows the start of retained history.
        try scrollLogsToTop(app)
        try snapshot("activity-retention")
        samples.append("activity_retention_seconds=\(started.duration(to: clock.now))")
        print("PASS: Logs records when older activity is removed")
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
               // The tab stays open without a conversation.
               conversations.isEmpty { return }
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
        } while clock.now < deadline
        throw Failure("Missing Codex conversation stayed in the Qrow workspace")
    }
    func waitSavedConversationTitle(_ expected: String) throws {
        let workspace = URL(fileURLWithPath: env["QROW_DATA_DIR"]!).appendingPathComponent("workspace.json")
        let deadline = clock.now.advanced(by: .seconds(10))
        repeat {
            if let data = try? Data(contentsOf: workspace),
               let content = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
               let assistant = content["assistant"] as? [String: Any],
               let conversations = assistant["conversations"] as? [[String: Any]],
               conversations.count == 1,
               conversations[0]["title"] as? String == expected,
               conversations[0]["title_source"] as? String == "codex" { return }
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
        } while clock.now < deadline
        throw Failure("Generated conversation title \(expected) was not saved")
    }
    func testAssistantRestart() throws {
        key(38, flags: .maskCommand) // Cmd+J opens the docked assistant.
        _ = try wait("Model: Synthetic Model", timeout: 20)
        if find("Assistant Message") == nil {
            try press("Back to Conversation")
        }
        // The previous process deleted the conversation of this tab.
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 1))
        try require(find("Codex cannot find this conversation") == nil, "A deleted conversation was restored after restart")
        try fill("Assistant Message", "Explain `SELECT 1` after restart")
        try press("Send")
        _ = try wait("I can help with this query", timeout: 20)
        try snapshot("assistant-after-restart")
        print("PASS: A tab without a conversation starts a new conversation after restart")
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
    /// Controls on a settings page share one left edge. A long description
    /// wraps and does not push its control past the edge of the page.
    func requireAlignedSetting(_ label: String, with reference: String) throws {
        let (expected, _) = try elementBounds(wait(reference, role: kAXPopUpButtonRole))
        let (actual, _) = try elementBounds(wait(label, role: kAXPopUpButtonRole))
        try require(
            abs(actual.x - expected.x) <= 1,
            "\(label) starts at x \(actual.x), not at the \(reference) edge \(expected.x)"
        )
    }
    func testSettings() throws {
        try selectApplicationMenuItem("Settings…")
        // Assistant typography is a section within Appearance.
        try waitSettingValue("Theme", "System")
        try waitSettingValue("UI Scale", "100")
        // At 110%, the description of the interface font is wider than its
        // column. It must wrap, and the picker must stay on the page.
        try press("Increase UI Scale")
        try waitSettingValue("UI Scale", "110")
        try requireAlignedSetting("UI Font Family", with: "Theme")
        try snapshot("settings-scaled")
        try press("Decrease UI Scale")
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
        // SQL Keyword Case is an Editor setting with a long description.
        try requireAlignedSetting("SQL Keyword Case", with: "Editor Font Family")
        // The search shows the setting at the top of the page, so the pointer
        // can reach it.
        try fill("Search...", "keyword")
        try waitSettingValue("SQL Keyword Case", "Uppercase")
        // Select rows have no AX bounds. Lowercase is the row below Uppercase.
        try click(try wait("SQL Keyword Case", role: kAXPopUpButtonRole))
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.3))
        key(125) // Down arrow
        key(36) // Return
        try waitSettingValue("SQL Keyword Case", "Lowercase")
        try fill("Search...", "logs")
        try waitSettingValue("Logs Font Size", "13")
        try selectFont("Logs Font Family", "System Font")
        try fill("Search...", "")
        try press("Restore defaults")
        try fill("Search...", "assistant")
        try waitSettingValue("Assistant Font Size", "14")
        try fill("Search...", "editor")
        try waitSettingValue("Editor Font Size", "13")
        try waitSettingValue("SQL Keyword Case", "Uppercase")
        try press("Save")
        try waitGone("UI Scale")
        print("PASS: Settings aligns controls beside long descriptions, applies all font pickers, changes sizes and keyword case, and restores defaults")
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
            if find("Codex Executable") != nil { break }
        }
        _ = try wait("Codex Executable", timeout: 5)
        try fill("Codex Executable", FileManager.default.currentDirectoryPath + "/tests/e2e/native/fake-codex.sh")
        try press("Enable Assistant")
        try activate(try waitExact("Enable", timeout: 10, role: kAXButtonRole))
        for _ in 0..<3 {
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.3))
            try press("Save")
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.3))
            if find("Codex Executable") == nil { break }
        }
        try waitGone("Codex Executable", timeout: 5)
        key(38, flags: .maskCommand) // Cmd+J opens the docked assistant.
        _ = try wait("Toggle Assistant")
        try require(find("New Connection") != nil, "Opening the assistant hid the Connections sidebar")
        if fontOnly {
            _ = try wait("Model: Synthetic Model", timeout: 20)
            if find("Search Conversations") == nil {
                try press("Toggle Conversation List")
            }
            let search = try wait("Search Conversations")
            let headerControl: AXUIElement
            if let toggle = find("Toggle Conversation List") {
                headerControl = toggle
            } else {
                headerControl = try wait("Back to Conversation")
            }
            let (searchPosition, searchSize) = try elementBounds(search)
            let (togglePosition, toggleSize) = try elementBounds(headerControl)
            try require(searchSize.height <= toggleSize.height + 1, "Conversation search input is taller than the header control")
            let searchCenter = searchPosition.y + searchSize.height / 2
            let toggleCenter = togglePosition.y + toggleSize.height / 2
            try require(abs(searchCenter - toggleCenter) <= 2, "Conversation search input is not aligned with the header")
            try snapshot("assistant-conversation-search-spacing")
            if find("Assistant Message") == nil {
                try press("Back to Conversation")
            }
            try fill("Assistant Message", "Explain `SELECT 1` in one sentence")
            try press("Send")
            _ = try wait("I can help with this query", timeout: 20)
            try snapshot("assistant-before-font-change")
            try verifyAssistantFontSwitch()
            try click(wait("Conversation Actions", role: kAXButtonRole))
            try pressMenuItem("Delete…")
            try press("Delete")
            try waitStaleConversationRemoved()
            print("PASS: Assistant fonts persist and a missing Codex conversation can be deleted")
            return
        }
        let startingModel = try wait("Model: waiting for Codex", timeout: 5, role: kAXButtonRole)
        try click(startingModel)
        try require(find("Synthetic Model") == nil, "Model picker opened while Codex started")
        for label in ["Reasoning: waiting for Codex", "Service tier: waiting for Codex", "Send · Ask"] {
            let control = try wait(label, timeout: 5, role: kAXButtonRole)
            try click(control)
            try require(find("Run automatically") == nil, "\(label) opened while Codex started")
        }
        try require(find("Starting Codex") == nil, "Startup status appeared above the message field")
        _ = try wait("Toggle Assistant")
        _ = try wait("New Conversation")
        _ = try wait("Toggle Conversation List")
        _ = try wait("Conversation Actions")
        let listInitiallyVisible = find("Search Conversations") != nil
        try press("Toggle Conversation List")
        if listInitiallyVisible {
            try waitGone("Search Conversations")
            try press("Toggle Conversation List")
        } else {
            _ = try wait("Search Conversations")
            try press("Back to Conversation")
        }
        let readyModel = try wait("Model: Synthetic Model", timeout: 20)
        try require(attribute(readyModel, kAXEnabledAttribute) as? Bool != false, "Model stayed disabled after Codex started")
        _ = try wait("Send · Ask", role: kAXButtonRole)
        try fill("Assistant Message", "Return to the latest message while hidden")
        try press("Send")
        _ = try wait("Assistant is working", timeout: 8)
        try requestWindowClose()
        _ = try wait("Keep Working", timeout: 10, role: kAXButtonRole)
        try require(process.isRunning, "Quit closed Qrow while Assistant was working")
        try press("Keep Working")
        try waitGone("Keep Working", timeout: 5)
        try press("Toggle Assistant")
        _ = try wait("Toggle Assistant, working", timeout: 5, role: kAXButtonRole)
        _ = try wait("Toggle Assistant, reply ready", timeout: 20, role: kAXButtonRole)
        try require(find("Done") == nil, "The assistant completion used the removed Done label")
        try press("Toggle Assistant, reply ready")
        _ = try wait("I can help with this query", timeout: 5)
        try fill("Assistant Message", "Keep this draft")
        try press("Run")
        let draft = attribute(try waitInput("Assistant Message"), kAXValueAttribute) as? String
        try require(draft == "Keep this draft", "Toolbar Run sent the assistant draft")
        try fill("Assistant Message", "Help me understand how `SELECT 1` behaves in the currently selected query tab and explain its result")
        key(36, flags: .maskCommand) // Cmd+Enter sends only in the composer.
        _ = try wait("I can help with this query", timeout: 20)
        try snapshot("assistant-markdown")
        try verifyAssistantFontSwitch()
        try fill("Assistant Message", "Write SELECT 1 into this tab")
        try press("Send")
        _ = try wait("I updated the SQL.", timeout: 20)
        try waitInputValue("SQL Editor", "SELECT 1")
        // Tool calls are full-width cards that start collapsed.
        let toolCard = try waitExact("Tool call: Append query", timeout: 5)
        try require(find("Arguments:") == nil, "The tool call opened expanded")
        try require(find("Tab: Query 1") == nil, "The collapsed tool call showed its tab")
        let (_, toolSize) = try elementBounds(toolCard)
        let (_, composerSize) = try elementBounds(try waitInput("Assistant Message"))
        try require(toolSize.width >= composerSize.width * 0.9, "The tool call card did not use the full transcript width")
        try snapshot("assistant")
        try activate(toolCard)
        _ = try wait("Arguments:", timeout: 5)
        _ = try wait("Tab: Query 1", timeout: 5)
        try snapshot("assistant-tool-expanded")
        try activate(try waitExact("Tool call: Append query", timeout: 5))
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
        try fill("Assistant Message", "Write SELECT 1 into this tab")
        try press("Send")
        try waitInputValue("SQL Editor", "SELECT 1")
        try waitGone("Assistant is working", timeout: 5)
        try fill("Assistant Message", "Write SELECT 2 into this tab")
        try press("Send")
        try waitInputValue("SQL Editor", "SELECT 1;\n\nSELECT 2")
        try fill("Assistant Message", "Show many lines")
        try press("Send")
        _ = try wait("Line 40", timeout: 20)
        try scrollUpAbove(waitInput("Assistant Message"))
        _ = try wait("Jump to Latest", timeout: 5)
        try snapshot("assistant-scrolled")
        try press("Jump to Latest")
        try waitGone("Jump to Latest", timeout: 5)
        // Revisit the wrapped user message after the transcript has been scrolled.
        for _ in 0..<4 { try scrollUpAbove(waitInput("Assistant Message")) }
        try snapshot("assistant-wrapped-after-scroll")
        try press("Jump to Latest")
        try waitGone("Jump to Latest", timeout: 5)
        try scrollUpAbove(waitInput("Assistant Message"))
        _ = try wait("Jump to Latest", timeout: 5)
        try fill("Assistant Message", "Return to the latest message")
        try press("Send")
        // The synthetic Codex server holds this turn open for 8 seconds. While
        // the indicator animates in a debug build, one accessibility tree scan
        // can take several seconds.
        _ = try wait("Assistant is working", timeout: 8)
        try snapshot("assistant-working")
        try waitGone("Assistant is working", timeout: 20)
        _ = try wait("I can help with this query", timeout: 5)
        try waitGone("Jump to Latest", timeout: 5)
        try press("Toggle Assistant")
        try waitGone("Toggle Conversation List")
        print("PASS: Assistant opt-in, docked chat, hidden-turn notification, keyboard routing, direct SQL edit, and Undo")
    }
    func testAssistantAppend() throws {
        key(38, flags: .maskCommand) // Cmd+J opens the docked assistant.
        _ = try wait("Model: Synthetic Model", timeout: 20)
        try fill("Assistant Message", "Write SELECT 1 into this tab")
        try press("Send")
        try waitInputValue("SQL Editor", "SELECT 1")
        try waitGone("Assistant is working", timeout: 5)
        // The first message starts title generation alongside the reply.
        try waitSavedConversationTitle("Title: Write SELECT 1")
        try snapshot("assistant-generated-title")
        try fill("Assistant Message", "Write SELECT 2 into this tab")
        try press("Send")
        try waitInputValue("SQL Editor", "SELECT 1;\n\nSELECT 2")
        try waitGone("Assistant is working", timeout: 5)
        // A generated title stays after later replies.
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 1))
        try waitSavedConversationTitle("Title: Write SELECT 1")
        // Qrow formats a long query when the assistant appends it. The comment
        // above the query stays as written and outside the selected statement.
        try fill("Assistant Message", "Write a long query into this tab")
        try press("Send")
        _ = try wait("I formatted the SQL.", timeout: 20)
        try waitInputValue(
            "SQL Editor",
            "SELECT 1;\n\nSELECT 2;\n\n-- Bookings by state\nSELECT\n  state,\n  COUNT(*) AS bookings,\n  MAX(booked_at) AS last_booked_at\nFROM integrations.bookings\nGROUP BY state;"
        )
        try waitGone("Assistant is working", timeout: 5)
        try snapshot("assistant-formatted-append")
        try setAssistantRunMode()
        // Reconnect replaces Send and Cancel in the composer while Codex is disconnected.
        try fill("Assistant Message", "Disconnect Codex")
        try press("Send")
        _ = try wait("Reconnect to Codex", timeout: 10, role: kAXButtonRole)
        try require(find("Send · Run") == nil, "Send stayed visible after Codex disconnected")
        try require(find("Cancel Assistant Turn") == nil, "Cancel stayed visible after Codex disconnected")
        try snapshot("assistant-disconnected")
        try press("Reconnect to Codex")
        try waitGone("Reconnect to Codex", timeout: 5)
        _ = try wait("Send · Run", timeout: 5, role: kAXButtonRole)
        try fill("Assistant Message", "Reply after reconnect")
        // The model label stays from the previous session, and GPUI Kit reports
        // Send as enabled while Codex starts. Retry until Qrow accepts the message.
        let deadline = clock.now.advanced(by: .seconds(20))
        repeat {
            try press("Send")
            if (try? waitInputValue("Assistant Message", "", timeout: 0.5)) != nil { break }
            try require(clock.now < deadline, "Qrow did not send a message after Reconnect")
        } while true
        _ = try wait("I can help with this query", timeout: 20)
        try waitGone("Assistant is working", timeout: 5)
        print("PASS: Assistant appends a new SQL query, preserves the first query, formats a long query below its comment, titles the conversation, and reconnects from the composer")
    }
    /// Waits until the saved workspace has these conversation titles and
    /// title sources, in any order.
    func waitSavedConversations(_ expected: [(String, String)]) throws {
        let workspace = URL(fileURLWithPath: env["QROW_DATA_DIR"]!).appendingPathComponent("workspace.json")
        let deadline = clock.now.advanced(by: .seconds(10))
        var saved: [[String: Any]] = []
        repeat {
            if let data = try? Data(contentsOf: workspace),
               let content = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
               let assistant = content["assistant"] as? [String: Any],
               let conversations = assistant["conversations"] as? [[String: Any]] {
                saved = conversations
                let titles = conversations.map { "\($0["title"] ?? "")|\($0["title_source"] ?? "")" }.sorted()
                if titles == expected.map({ "\($0.0)|\($0.1)" }).sorted() { return }
            }
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
        } while clock.now < deadline
        throw Failure("Saved conversations \(saved) did not match \(expected)")
    }
    /// A thread list row names the conversation and its connection.
    func conversationRow(_ title: String) -> String { "\(title), " }
    func openConversationMenu(_ title: String) throws {
        try contextMenu(conversationRow(title), role: kAXButtonRole)
    }
    /// The conversation header menu and the thread list context menu rename,
    /// regenerate titles, and delete conversations. Both renames use the
    /// query tab rename dialog. Duplicate titles do not show Codex thread IDs.
    func testAssistantConversationTitles() throws {
        key(38, flags: .maskCommand) // Cmd+J opens the docked assistant.
        _ = try wait("Model: Synthetic Model", timeout: 20)
        if find("Assistant Message") == nil {
            try press("Back to Conversation")
        }
        let generated = "Title: Count sandbox schemas"
        try fill("Assistant Message", "Count sandbox schemas now")
        try press("Send")
        _ = try wait("I can help with this query", timeout: 20)
        try waitSavedConversations([(generated, "codex")])

        try click(wait("Conversation Actions", role: kAXButtonRole))
        try pressMenuItem("Rename…")
        let nameField = try wait("Conversation Name", role: kAXTextFieldRole)
        try require(
            attribute(nameField, kAXValueAttribute) as? String == generated,
            "Rename form did not prefill the current conversation title"
        )
        // An empty name keeps the dialog open. GPUI does not publish the
        // validation text in the macOS accessibility tree.
        try fill("Conversation Name", "")
        try press("Rename")
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.5))
        _ = try wait("Conversation Name", role: kAXTextFieldRole)
        try waitSavedConversations([(generated, "codex")])
        try snapshot("assistant-rename-dialog-error")
        try fill("Conversation Name", "Custom title")
        try press("Rename")
        try waitGone("Conversation Name", timeout: 5)
        try waitSavedConversations([("Custom title", "user")])
        try click(wait("Conversation Actions", role: kAXButtonRole))
        _ = try wait("Regenerate Title", timeout: 5)
        try snapshot("assistant-header-menu")
        try pressMenuItem("Regenerate Title")
        try waitSavedConversations([(generated, "codex")])

        try press("New Conversation")
        try fill("Assistant Message", "Count sandbox schemas again")
        try press("Send")
        _ = try wait("I can help with this query", timeout: 20)
        try waitSavedConversations([(generated, "codex"), (generated, "codex")])
        if find("Search Conversations") == nil {
            try press("Toggle Conversation List")
        }
        _ = try wait(conversationRow(generated), timeout: 10, role: kAXButtonRole)
        try require(find("\(generated) ·") == nil, "The thread list showed a Codex thread ID")
        try snapshot("assistant-duplicate-titles")
        // A narrow pane swaps the list for the selected conversation.
        try click(wait(conversationRow(generated), timeout: 10, role: kAXButtonRole))
        try waitGone("Search Conversations")
        _ = try wait("Assistant Message")
        try press("Toggle Conversation List")

        try openConversationMenu(generated)
        try pressMenuItem("Rename…")
        _ = try waitInput("Conversation Name")
        try snapshot("assistant-rename-dialog")
        try press("Cancel")
        try waitGone("Conversation Name", timeout: 5)
        try openConversationMenu(generated)
        try pressMenuItem("Rename…")
        try fill("Conversation Name", "Listed title")
        try press("Rename")
        try waitGone("Conversation Name", timeout: 5)
        _ = try wait(conversationRow("Listed title"), timeout: 10, role: kAXButtonRole)
        try waitSavedConversations([(generated, "codex"), ("Listed title", "user")])
        try snapshot("assistant-list-renamed")
        try openConversationMenu("Listed title")
        _ = try wait("Regenerate Title", timeout: 5)
        try snapshot("assistant-list-menu")
        try pressMenuItem("Regenerate Title")
        try waitSavedConversations([(generated, "codex"), (generated, "codex")])
        try waitGone("Listed title", timeout: 10, role: kAXButtonRole)
        try openConversationMenu(generated)
        try pressMenuItem("Delete…")
        // macOS Accessibility does not expose AlertDialog body text for this
        // component. Keep a screenshot as the visual copy assertion.
        try snapshot("assistant-delete-dialog")
        try press("Delete")
        try waitSavedConversations([(generated, "codex")])
        print("PASS: Assistant conversation menus rename, regenerate titles, and delete conversations without thread IDs in the list")
    }
    func seedAssistantStatementWorkspace() throws {
        let workspace = URL(fileURLWithPath: env["QROW_DATA_DIR"]!).appendingPathComponent("workspace.json")
        try require(!FileManager.default.fileExists(atPath: workspace.path), "The statement check needs an empty workspace directory")
        let profileID = UUID().uuidString.lowercased()
        let tabID = UUID().uuidString.lowercased()
        let data = try JSONSerialization.data(withJSONObject: [
            "version": 3,
            "settings": [
                // A style that differs from the defaults.
                "editor_tab_size": 4,
                "assistant": [
                    "enabled": true,
                    "data_sharing_notice_version": 1,
                    "codex_executable": FileManager.default.currentDirectoryPath + "/tests/e2e/native/fake-codex.sh",
                    "sql_keyword_case": "lowercase",
                ],
            ],
            "profiles": [[
                "id": profileID, "name": "Synthetic", "host": "example.invalid",
                "port": 10009, "username": "synthetic", "database": "default",
                "parameters": [:],
            ]],
            "tabs": [["id": tabID, "title": "Query 1", "sql": "SELECT 0;", "profile": profileID]],
            "active_tab": 0,
        ])
        try data.write(to: workspace)
    }
    func testAssistantStatementRun() throws {
        try waitInputValue("SQL Editor", "SELECT 0;")
        key(38, flags: .maskCommand) // Cmd+J opens the docked assistant.
        _ = try wait("Model: Synthetic Model", timeout: 20)
        try fill("Assistant Message", "Append two SQL statements with edit tool")
        try press("Send")
        try waitInputValue("SQL Editor", "SELECT 0;\n\nSELECT 1;\n\nSELECT 2")
        try waitGone("Assistant is working", timeout: 5)

        try fill("Assistant Message", "Run selected SQL without range")
        try press("Send")
        _ = try wait("Run in Query 1 · Synthetic? SELECT 2", timeout: 20)
        try activate(try waitExact("Cancel", timeout: 5, role: kAXButtonRole))
        try waitGone("Run in ", timeout: 5)
        try waitGone("Assistant is working", timeout: 5)

        try fill("Assistant Message", "Run first SQL by range")
        try press("Send")
        _ = try wait("Run in Query 1 · Synthetic? SELECT 0;", timeout: 20)
        try activate(try waitExact("Cancel", timeout: 5, role: kAXButtonRole))
        try waitGone("Run in ", timeout: 5)
        try waitGone("Assistant is working", timeout: 5)

        // Qrow formats a long statement that an edit supplies, in the saved
        // keyword case and tab size.
        try fill("Assistant Message", "Rewrite the last statement with edit tool")
        try press("Send")
        _ = try wait("I formatted the SQL.", timeout: 20)
        try waitInputValue(
            "SQL Editor",
            "SELECT 0;\n\nSELECT 1;\n\nselect\n    state,\n    count(*) as bookings,\n    max(booked_at) as last_booked_at\nfrom integrations.bookings\ngroup by state"
        )
        try waitGone("Assistant is working", timeout: 5)
        try snapshot("assistant-formatted-edit")

        // A change in Settings applies to the next message of the conversation.
        try selectApplicationMenuItem("Settings…")
        try fill("Search...", "tab size")
        try waitSettingValue("Editor Tab Size", "4")
        try press("Increase Editor Tab Size")
        try waitSettingValue("Editor Tab Size", "5")
        try snapshot("settings-tab-size")
        try fill("Search...", "keyword")
        try waitSettingValue("SQL Keyword Case", "Lowercase")
        // Select rows have no AX bounds. Uppercase is the row above Lowercase.
        try click(try wait("SQL Keyword Case", role: kAXPopUpButtonRole))
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.3))
        key(126) // Up arrow
        key(36) // Return
        try waitSettingValue("SQL Keyword Case", "Uppercase")
        try snapshot("settings-sql-style")
        try press("Save")
        try waitGone("Editor Tab Size")
        try fill("Assistant Message", "Report the SQL style")
        try press("Send")
        _ = try wait("SQL style: uppercase, 5 spaces", timeout: 20)
        try waitGone("Assistant is working", timeout: 5)
        print("PASS: Assistant can target the newest and an earlier statement for execution, a rewritten long statement uses the saved SQL style, and Settings changes the style")
    }
    func seedAssistantRetargetWorkspace() throws {
        let workspace = URL(fileURLWithPath: env["QROW_DATA_DIR"]!).appendingPathComponent("workspace.json")
        try require(!FileManager.default.fileExists(atPath: workspace.path), "The retarget check needs an empty workspace directory")
        let profileID = UUID().uuidString.lowercased()
        let data = try JSONSerialization.data(withJSONObject: [
            "version": 3,
            "settings": ["assistant": [
                "enabled": true,
                "data_sharing_notice_version": 1,
                "codex_executable": FileManager.default.currentDirectoryPath + "/tests/e2e/native/fake-codex.sh",
            ]],
            "profiles": [[
                "id": profileID, "name": "Synthetic", "host": "example.invalid",
                "port": 10009, "username": "synthetic", "database": "default",
                "parameters": [:],
            ]],
            "tabs": [
                ["id": UUID().uuidString.lowercased(), "title": "Query 1", "sql": "SELECT 1;", "profile": profileID],
                ["id": UUID().uuidString.lowercased(), "title": "Query 2", "sql": "SELECT 99;", "profile": profileID],
            ],
            "active_tab": 0,
        ])
        try data.write(to: workspace)
    }
    /// Writes the layout workspace and starts the synthetic Codex server without an account.
    func seedAssistantSignInWorkspace() throws {
        try seedAssistantLayoutWorkspace()
        let state = URL(fileURLWithPath: env["QROW_DATA_DIR"]!).appendingPathComponent("fake-codex")
        try FileManager.default.createDirectory(at: state, withIntermediateDirectories: true)
        try Data().write(to: state.appendingPathComponent("signed-out"))
    }
    /// The sign-in screen replaces the transcript. A failed sign-in shows the
    /// Codex error until Codex has an account, and then the conversation opens.
    func testAssistantSignIn() throws {
        key(38, flags: .maskCommand) // Cmd+J opens the docked assistant.
        _ = try wait("Sign in with ChatGPT…", timeout: 20, role: kAXButtonRole)
        try snapshot("assistant-sign-in")
        try press("Sign in with ChatGPT…")
        _ = try wait("Couldn't sign in. ", timeout: 10)
        _ = try wait("port 1455 is in use", timeout: 5)
        try snapshot("assistant-sign-in-error")

        let marker = URL(fileURLWithPath: env["QROW_DATA_DIR"]!).appendingPathComponent("fake-codex/sign-in-elsewhere")
        try Data().write(to: marker)
        try waitGone("Couldn't sign in. ", timeout: 10)
        try waitGone("Sign in with ChatGPT…", timeout: 5)
        try fill("Assistant Message", "Hello after sign-in")
        try press("Send")
        _ = try wait("I can help with this query", timeout: 20)
        print("PASS: Assistant sign-in shows its error until Codex has an account, then opens the conversation")
    }
    /// A wide pane keeps a reopened thread list open when you select a
    /// conversation in it.
    func testAssistantThreadList() throws {
        key(38, flags: .maskCommand) // Cmd+J opens the docked assistant.
        _ = try wait("Model: Synthetic Model", timeout: 20)
        key(11, flags: .maskCommand) // Cmd+B hides the Connections sidebar.
        try waitGone("New Connection")
        guard (try? wait("Search Conversations", timeout: 10)) != nil, find("Back to Conversation") == nil else {
            try require(process.isRunning, "Qrow exited while waiting for Search Conversations")
            throw Failure("The assistant pane is narrow. Use a larger main display")
        }
        for (index, message) in ["Count sandbox schemas now", "Count sandbox schemas again"].enumerated() {
            if index > 0 { try press("New Conversation") }
            try fill("Assistant Message", message)
            try press("Send")
            _ = try wait("I can help with this query", timeout: 20)
        }
        let generated = "Title: Count sandbox schemas"
        try waitSavedConversations([(generated, "codex"), (generated, "codex")])
        try press("Toggle Conversation List")
        try waitGone("Search Conversations")
        try press("Toggle Conversation List")
        _ = try wait("Search Conversations")
        try click(wait(conversationRow(generated), timeout: 10, role: kAXButtonRole))
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.5))
        try require(find("Search Conversations") != nil, "Selecting a conversation closed the list on a wide pane")
        _ = try wait("Assistant Message")
        try snapshot("assistant-thread-list-after-selection")
        print("PASS: A wide Assistant pane keeps a reopened conversation list open after a selection")
    }
    /// A conversation keeps its query tab when you select and rename another
    /// tab during a turn. An unknown tab ID in a tool call uses the
    /// conversation tab.
    func testAssistantRetargetAfterRename() throws {
        try waitInputValue("SQL Editor", "SELECT 1;")
        key(38, flags: .maskCommand) // Cmd+J opens the docked assistant.
        _ = try wait("Model: Synthetic Model", timeout: 20)
        try fill("Assistant Message", "Run tab selected after rename")
        try press("Send")

        try contextMenu("Query 2", exact: true)
        try pressMenuItem("Rename…")
        _ = try wait("Tab Name", role: kAXTextFieldRole)
        try fill("Tab Name", "Default")
        try press("Rename")
        try waitGone("Tab Name")
        try click(try waitExact("Default"))
        try waitInputValue("SQL Editor", "SELECT 99;")
        let marker = URL(fileURLWithPath: env["QROW_DATA_DIR"]!).appendingPathComponent("fake-codex/retarget-ready")
        try Data().write(to: marker)

        // The approval waits in the conversation tab. The pane of the
        // selected tab does not show it.
        let owner = try wait("Query 1, assistant waiting for approval", timeout: 20, role: kAXRadioButtonRole)
        _ = try wait("Toggle Assistant, waiting for approval", timeout: 5, role: kAXButtonRole)
        try require(find("Run in ") == nil, "The approval showed in a tab without the conversation")
        try snapshot("assistant-retarget-background-approval")
        try click(owner)
        _ = try wait("Run in Query 1 · Synthetic? SELECT 1;", timeout: 10)
        try waitInputValue("SQL Editor", "SELECT 1;")
        try activate(try waitExact("Cancel", timeout: 5, role: kAXButtonRole))
        print("PASS: Assistant keeps its query tab when you select and rename another tab during a turn")
    }
    /// A tool call that names another open tab still appends to the tab of
    /// its conversation, even while the other tab is on screen.
    func testAssistantWrongActionTab() throws {
        try waitInputValue("SQL Editor", "SELECT 1;")
        key(38, flags: .maskCommand)
        _ = try wait("Model: Synthetic Model", timeout: 20)
        try showConversation()
        try fill("Assistant Message", "Write query with another tab ID")
        try press("Send")
        try selectConnection("Beta")
        try waitInputValue("SQL Editor", "")
        let marker = URL(fileURLWithPath: env["QROW_DATA_DIR"]!)
            .appendingPathComponent("fake-codex/wrong-tab-ready")
        try FileManager.default.createDirectory(at: marker.deletingLastPathComponent(),
                                                withIntermediateDirectories: true)
        try Data().write(to: marker)
        try waitSaved("a mismatched tool ID writes only to the conversation tab") { workspace in
            let tabs = savedTabs(workspace)
            let alpha = savedProfileID(workspace, "Alpha")
            let beta = savedProfileID(workspace, "Beta")
            return tabs.contains {
                $0["profile"] as? String == alpha
                    && $0["sql"] as? String == "SELECT 1;\n\n-- Tables in dwh_meta\nSHOW TABLES IN dwh_meta"
            } && tabs.contains {
                $0["profile"] as? String == beta && $0["sql"] as? String == ""
            }
        }
        try selectConnection("Alpha")
        _ = try wait("I updated the SQL.", timeout: 20)
        try require(find("Failed") == nil, "The append tool failed after using the other tab ID")
        try snapshot("assistant-wrong-action-tab")
        print("PASS: Assistant appends to its conversation tab when a tool names another open tab and connection")
    }
    /// Shows the thread list of a narrow pane. A selection in the list can
    /// still be closing it, so retry until the list shows.
    func showConversationList() throws {
        let deadline = clock.now.advanced(by: .seconds(10))
        repeat {
            if find("Search Conversations") != nil { return }
            if let toggle = find("Toggle Conversation List", role: kAXButtonRole) { try activate(toggle) }
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.5))
        } while clock.now < deadline
        throw Failure("The conversation list did not open")
    }
    /// Shows the conversation of a narrow pane. A selection in the list
    /// closes the list by itself.
    func showConversation() throws {
        let deadline = clock.now.advanced(by: .seconds(10))
        repeat {
            if accessibleInput("Assistant Message") != nil { return }
            if let back = find("Back to Conversation", role: kAXButtonRole) { try activate(back) }
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.5))
        } while clock.now < deadline
        throw Failure("The conversation did not show")
    }
    func savedWorkspace() -> [String: Any]? {
        let url = URL(fileURLWithPath: env["QROW_DATA_DIR"]!).appendingPathComponent("workspace.json")
        guard let data = try? Data(contentsOf: url) else { return nil }
        return try? JSONSerialization.jsonObject(with: data) as? [String: Any]
    }
    /// Waits until the saved workspace meets a condition.
    func waitSaved(_ description: String, timeout: Double = 10, _ condition: ([String: Any]) -> Bool) throws {
        let deadline = clock.now.advanced(by: .seconds(timeout))
        repeat {
            if let workspace = savedWorkspace(), condition(workspace) { return }
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
        } while clock.now < deadline
        throw Failure("The saved workspace does not show that \(description): \(savedWorkspace().map { "\($0)" } ?? "none")")
    }
    func savedConversations(_ workspace: [String: Any]) -> [[String: Any]] {
        ((workspace["assistant"] as? [String: Any])?["conversations"] as? [[String: Any]]) ?? []
    }
    func savedProfileID(_ workspace: [String: Any], _ name: String) -> String? {
        ((workspace["profiles"] as? [[String: Any]]) ?? []).first { $0["name"] as? String == name }?["id"] as? String
    }
    func savedTabs(_ workspace: [String: Any]) -> [[String: Any]] {
        (workspace["tabs"] as? [[String: Any]]) ?? []
    }
    /// Writes a workspace with the connections Alpha and Beta and one tab for
    /// each. The narrow pane shows the thread list or a conversation.
    func seedAssistantConnectionsWorkspace(alphaSQL: String) throws {
        let workspace = URL(fileURLWithPath: env["QROW_DATA_DIR"]!).appendingPathComponent("workspace.json")
        try require(!FileManager.default.fileExists(atPath: workspace.path), "The check needs an empty workspace directory")
        let profiles = ["Alpha", "Beta"].map { name -> [String: Any] in
            ["id": UUID().uuidString.lowercased(), "name": name, "host": "example.invalid",
             "port": 10009, "username": "synthetic", "database": "default", "parameters": [:]]
        }
        let tabs = profiles.enumerated().map { index, profile -> [String: Any] in
            ["id": UUID().uuidString.lowercased(), "title": "Query 1",
             "sql": index == 0 ? alphaSQL : "", "profile": profile["id"]!]
        }
        let data = try JSONSerialization.data(withJSONObject: [
            "version": 4,
            "settings": ["assistant": [
                "enabled": true,
                "data_sharing_notice_version": 1,
                "codex_executable": FileManager.default.currentDirectoryPath + "/tests/e2e/native/fake-codex.sh",
                "panel_width": 536,
            ]],
            "profiles": profiles,
            "tabs": tabs,
            "active_tab": 0,
        ])
        try data.write(to: workspace)
    }
    /// Two conversations on two connections work at the same time. Each one
    /// appends and requests SQL in its own tab while you work in the other.
    /// The thread list, the tabs, and the assistant toggle show their states.
    func testAssistantParallel() throws {
        key(38, flags: .maskCommand) // Cmd+J opens the docked assistant.
        _ = try wait("Model: Synthetic Model", timeout: 20)
        try showConversation()
        try fill("Assistant Message", "Hold parallel Alpha")
        try press("Send")
        _ = try wait("Assistant is working", timeout: 10)
        _ = try wait("Query 1, assistant working", timeout: 5, role: kAXRadioButtonRole)
        try selectConnection("Beta")
        // The Beta tab has no conversation, so its pane does not wait.
        try waitGone("Assistant is working", timeout: 5)
        try fill("Assistant Message", "Hold parallel Beta")
        try press("Send")
        _ = try wait("Assistant is working", timeout: 10)
        try showConversationList()
        _ = try wait("Alpha, assistant working", timeout: 10, role: kAXButtonRole)
        _ = try wait("Beta, assistant working", timeout: 10, role: kAXButtonRole)
        try snapshot("assistant-parallel-working")

        let state = URL(fileURLWithPath: env["QROW_DATA_DIR"]!).appendingPathComponent("fake-codex")
        for label in ["Alpha", "Beta"] {
            try Data().write(to: state.appendingPathComponent("release-\(label)"))
        }
        _ = try wait("Alpha, assistant waiting for approval", timeout: 20, role: kAXButtonRole)
        _ = try wait("Beta, assistant waiting for approval", timeout: 20, role: kAXButtonRole)
        _ = try wait("Toggle Assistant, waiting for approval", timeout: 5, role: kAXButtonRole)
        try showConversation()
        // Each conversation changed only its own tab.
        try waitInputValue("SQL Editor", "SELECT 22")
        _ = try wait("Run in Query 1 · Beta? SELECT 22", timeout: 10)
        try snapshot("assistant-parallel-approval")
        try activate(try waitExact("Cancel", timeout: 5, role: kAXButtonRole))
        try waitGone("Run in ", timeout: 5)

        // Select the other conversation before the Beta turn ends. Qrow
        // selects its tab and connection.
        try showConversationList()
        try click(wait("Alpha, assistant waiting for approval", timeout: 5, role: kAXButtonRole))
        try showConversation()
        try waitInputValue("SQL Editor", "SELECT 11")
        _ = try wait("Run in Query 1 · Alpha? SELECT 11", timeout: 10)
        try showConversationList()
        _ = try wait("Beta, assistant reply ready", timeout: 15, role: kAXButtonRole)
        try snapshot("assistant-parallel-reply-ready")
        try showConversation()
        try activate(try waitExact("Cancel", timeout: 5, role: kAXButtonRole))
        _ = try wait("Finished Alpha: approval_cancelled", timeout: 15)
        _ = try wait("Toggle Assistant, reply ready", timeout: 5, role: kAXButtonRole)
        try showConversationList()
        try click(wait("Beta, assistant reply ready", timeout: 5, role: kAXButtonRole))
        try showConversation()
        _ = try wait("Finished Beta: approval_cancelled", timeout: 10)
        _ = try waitExact("Toggle Assistant", timeout: 5, role: kAXButtonRole)
        try require(
            !FileManager.default.fileExists(atPath: state.appendingPathComponent("duplicate-answer").path),
            "Qrow answered a replayed tool call twice"
        )
        try waitSaved("each conversation owns the tab of its connection") { workspace in
            let tabs = savedTabs(workspace)
            func tab(_ name: String) -> [String: Any]? {
                let profile = savedProfileID(workspace, name)
                return tabs.first { $0["profile"] as? String == profile }
            }
            guard let alpha = tab("Alpha"), let beta = tab("Beta"),
                  alpha["sql"] as? String == "SELECT 11", beta["sql"] as? String == "SELECT 22" else { return false }
            let linked = Set(savedConversations(workspace).compactMap { $0["tab_id"] as? String })
            return linked == Set([alpha["id"] as? String, beta["id"] as? String].compactMap { $0 })
        }
        print("PASS: Two Assistant conversations work at the same time in their own tabs, a replayed tool call runs once, and the list, tabs, and toggle show their states")
    }
    /// New Conversation names its tab after the generated thread title. A
    /// failed title keeps the default tab name, and a user tab rename wins.
    func testAssistantTabTitles() throws {
        key(38, flags: .maskCommand) // Cmd+J opens the docked assistant.
        _ = try wait("Model: Synthetic Model", timeout: 20)
        try showConversation()
        key(11, flags: .maskCommand) // Keep four tab titles visible for the menu check.
        try waitGone("New Connection", timeout: 5)
        let title = "Title: Name sales report"
        try press("New Conversation")
        _ = try waitExact("Query 2", timeout: 5, role: kAXRadioButtonRole)
        try fill("Assistant Message", "Hold title generation")
        try press("Send")
        _ = try wait("I can help with this query", timeout: 20)
        let titleState = URL(fileURLWithPath: env["QROW_DATA_DIR"]!)
            .appendingPathComponent("fake-codex")
        let pending = titleState.appendingPathComponent("title-generation-pending")
        let pendingDeadline = clock.now.advanced(by: .seconds(20))
        while !FileManager.default.fileExists(atPath: pending.path) {
            try require(clock.now < pendingDeadline, "The synthetic title request did not start")
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
        }
        _ = try wait("generating title", timeout: 10, role: kAXRadioButtonRole)
        _ = try wait("Generating title for New conversation", timeout: 5)
        try snapshot("assistant-title-generating")
        try showConversationList()
        _ = try wait("generating title", timeout: 5, role: kAXButtonRole)
        try snapshot("assistant-list-title-generating")
        try showConversation()
        try Data().write(to: titleState.appendingPathComponent("title-generation-release"))
        _ = try waitExact("Title: Hold title generation", timeout: 20, role: kAXRadioButtonRole)
        try waitGone("generating title", timeout: 5, role: kAXRadioButtonRole)

        // The title can finish while the first assistant reply is still pending.
        // The tab and pane header must show the same conversation title.
        try press("New Conversation")
        _ = try waitExact("Query 2", timeout: 5, role: kAXRadioButtonRole)
        try fill("Assistant Message", "Title before first reply")
        try press("Send")
        _ = try wait("Assistant is working", timeout: 10)
        _ = try wait("Title: Title before first", timeout: 20, role: kAXRadioButtonRole)
        _ = try waitExact("Conversation title: Title: Title before first", timeout: 5)
        try snapshot("assistant-title-before-reply")
        try Data().write(to: titleState.appendingPathComponent("first-reply-release"))
        _ = try wait("I can help with this query", timeout: 20)
        try press("Close Title: Title before first")

        for expected in [title, "\(title) (Copy)"] {
            try press("New Conversation")
            _ = try waitExact("Query 2", timeout: 5, role: kAXRadioButtonRole)
            try fill("Assistant Message", "Name sales report")
            try press("Send")
            _ = try wait("I can help with this query", timeout: 20)
            _ = try waitExact(expected, timeout: 20, role: kAXRadioButtonRole)
        }
        try waitSaved("both generated titles name their own tabs") { workspace in
            let alpha = savedProfileID(workspace, "Alpha")
            let tabs = savedTabs(workspace).filter { $0["profile"] as? String == alpha }
            let names = Set(tabs.compactMap { $0["title"] as? String })
            let conversations = savedConversations(workspace).filter { $0["title"] as? String == title }
            return names.contains(title) && names.contains("\(title) (Copy)")
                && conversations.count == 2
                && conversations.allSatisfy { $0["title_source"] as? String == "codex" }
        }
        try click(wait("Conversation Actions", role: kAXButtonRole))
        try pressMenuItem("Rename…")
        try fill("Conversation Name", "Sales summary")
        try press("Rename")
        try waitGone("Conversation Name", timeout: 5)
        _ = try waitExact("Sales summary", timeout: 10, role: kAXRadioButtonRole)
        try click(wait("Conversation Actions", role: kAXButtonRole))
        try pressMenuItem("Regenerate Title")
        _ = try waitExact("\(title) (Copy)", timeout: 20, role: kAXRadioButtonRole)

        try press("New Conversation")
        _ = try waitExact("Query 2", timeout: 5, role: kAXRadioButtonRole)
        try fill("Assistant Message", "Fail title generation")
        try press("Send")
        _ = try wait("I can help with this query", timeout: 20)
        let failure = URL(fileURLWithPath: env["QROW_DATA_DIR"]!)
            .appendingPathComponent("fake-codex/title-failure-once")
        let deadline = clock.now.advanced(by: .seconds(20))
        while !FileManager.default.fileExists(atPath: failure.path) {
            try require(clock.now < deadline, "The synthetic title failure did not run")
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
        }
        try waitSaved("a failed title keeps the default tab name") { workspace in
            let alpha = savedProfileID(workspace, "Alpha")
            guard let tab = savedTabs(workspace).first(where: {
                $0["profile"] as? String == alpha && $0["title"] as? String == "Query 2"
            }), let id = tab["id"] as? String else { return false }
            return savedConversations(workspace).contains {
                $0["tab_id"] as? String == id
                    && $0["title_source"] as? String == "temporary"
            }
        }
        try waitGone("generating title", timeout: 5, role: kAXRadioButtonRole)
        key(38, flags: .maskCommand) // Show the full tab strip for the rename menu.
        try waitGone("Assistant Message", timeout: 5)
        try contextMenu("Query 2", exact: true)
        try pressMenuItem("Rename…")
        try fill("Tab Name", "My SQL")
        try press("Rename")
        try waitGone("Tab Name", timeout: 5)
        _ = try waitExact("My SQL", timeout: 5, role: kAXRadioButtonRole)
        try press("Toggle Assistant")
        _ = try waitInput("Assistant Message")
        try fill("Assistant Message", "Continue the report")
        try press("Send")
        try waitSaved("a later generated title keeps the user tab name") { workspace in
            let alpha = savedProfileID(workspace, "Alpha")
            guard let tab = savedTabs(workspace).first(where: {
                $0["profile"] as? String == alpha && $0["title"] as? String == "My SQL"
            }), let id = tab["id"] as? String else { return false }
            return savedConversations(workspace).contains {
                $0["tab_id"] as? String == id
                    && $0["title"] as? String == "Title: Fail title generation"
                    && $0["title_source"] as? String == "codex"
            }
        }

        print("PASS: New conversation tabs use short generated titles, keep names unique, retain defaults on title failure, preserve user tab names, and shimmer while titles generate")
    }
    /// A conversation starts in a tab with SQL and stays in the list when its
    /// tab closes. Its next message opens a new tab. The conversation moves
    /// with its tab and leaves the tab open when you delete it.
    func testAssistantTabBinding() throws {
        try waitInputValue("SQL Editor", "SELECT 5;")
        key(38, flags: .maskCommand) // Cmd+J opens the docked assistant.
        _ = try wait("Model: Synthetic Model", timeout: 20)
        try showConversation()
        try fill("Assistant Message", "Report the tab SQL")
        try press("Send")
        _ = try wait("Tab SQL: SELECT 5;", timeout: 20)
        try waitGone("Assistant is working", timeout: 5)
        try waitSavedConversationTitle("Title: Report the tab")
        var firstTab = ""
        try waitSaved("the conversation owns its first tab") { workspace in
            let alpha = savedProfileID(workspace, "Alpha")
            guard let tab = savedTabs(workspace).first(where: { $0["profile"] as? String == alpha }),
                  let id = tab["id"] as? String,
                  savedConversations(workspace).first?["tab_id"] as? String == id else { return false }
            firstTab = id
            return true
        }

        try press("Close Query 1")
        try waitSaved("a closed tab detaches its conversation") { workspace in
            guard let conversation = savedConversations(workspace).first else { return false }
            return conversation["tab_id"] is NSNull
                && conversation["detached_profile"] as? String == savedProfileID(workspace, "Alpha")
                && !savedTabs(workspace).contains { $0["id"] as? String == firstTab }
        }
        try showConversationList()
        try click(wait("Alpha · Tab closed", timeout: 10, role: kAXButtonRole))
        try showConversation()
        _ = try wait("Tab SQL: SELECT 5;", timeout: 10)
        try waitSaved("browsing a closed conversation leaves it detached") { workspace in
            savedConversations(workspace).first?["tab_id"] is NSNull
                && savedTabs(workspace).allSatisfy { $0["title"] as? String != "Title: Report the tab" }
        }
        try fill("Assistant Message", "Continue the report")
        try press("Send")
        _ = try wait("I can help with this query", timeout: 20)
        var reopened = ""
        let reopenedTitle = "Title: Report the tab"
        try waitSaved("the conversation opens in a new tab under Alpha") { workspace in
            let alpha = savedProfileID(workspace, "Alpha")
            guard let id = savedConversations(workspace).first?["tab_id"] as? String,
                  let tab = savedTabs(workspace).first(where: { $0["id"] as? String == id }),
                  tab["profile"] as? String == alpha, tab["title"] as? String == reopenedTitle else { return false }
            reopened = id
            return true
        }
        try snapshot("assistant-reopened-tab")

        // Keyboard navigation opens the Move submenu. Beta is its only item.
        try contextMenu(reopenedTitle, exact: true)
        _ = try wait("Move to Connection…")
        for _ in 0..<4 { key(125) }
        try enterSubmenu("Move to Connection…", showing: "Beta")
        key(36)
        try waitGone("Move to Connection…")
        try waitSaved("the conversation moves with its tab") { workspace in
            let beta = savedProfileID(workspace, "Beta")
            return savedConversations(workspace).first?["tab_id"] as? String == reopened
                && savedTabs(workspace).contains { $0["id"] as? String == reopened && $0["profile"] as? String == beta }
        }
        try showConversationList()
        _ = try wait(", Beta", timeout: 10, role: kAXButtonRole)
        try showConversation()
        // A tab with a conversation cannot start another one.
        try contextMenu(reopenedTitle, exact: true)
        // GPUI Kit does not publish the disabled state of a menu item. A
        // disabled item ignores the press, so the menu stays open.
        try activate(try wait("Start Conversation", timeout: 5, role: kAXMenuItemRole))
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.5))
        try require(find("Start Conversation", role: kAXMenuItemRole) != nil, "Start Conversation was available in a tab with a conversation")
        key(53) // Escape closes the menu.
        try waitGone("Start Conversation", timeout: 5)

        try click(wait("Conversation Actions", role: kAXButtonRole))
        try pressMenuItem("Delete…")
        try press("Delete")
        try waitSaved("the tab stays after its conversation is deleted") { workspace in
            savedConversations(workspace).isEmpty
                && savedTabs(workspace).contains { $0["id"] as? String == reopened }
        }

        // The tab menu starts a conversation in a tab without one.
        try press("Toggle Assistant")
        try waitGone("Assistant Message", timeout: 5)
        try fill("SQL Editor", "SELECT 7;")
        try contextMenu(reopenedTitle, exact: true)
        let start = try wait("Start Conversation", timeout: 5, role: kAXMenuItemRole)
        try snapshot("assistant-tab-menu")
        try activate(start)
        try waitGone("Start Conversation", timeout: 5)
        try showConversation()
        try fill("Assistant Message", "Report the tab SQL")
        try press("Send")
        _ = try wait("Tab SQL: SELECT 7;", timeout: 20)
        try waitSaved("the tab menu starts a conversation in this tab") { workspace in
            savedConversations(workspace).first?["tab_id"] as? String == reopened
        }
        print("PASS: An Assistant conversation starts in a tab with SQL, stays detached while browsed after its tab closes, opens a new tab on the next message, moves with its tab, and leaves its tab when deleted. The tab menu starts a conversation only in a tab without one")
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
                "data_sharing_notice_version": 1,
                "codex_executable": FileManager.default.currentDirectoryPath + "/tests/e2e/native/fake-codex.sh",
                "panel_width": panelWidth,
            ],
        ]
        if let theme { settings["theme"] = theme }
        let data = try JSONSerialization.data(withJSONObject: [
            "version": 3, "settings": settings, "profiles": [], "tabs": [], "active_tab": 0,
        ])
        try data.write(to: workspace)
    }
    /// A message with inline code shows its last word. Lines with bold text
    /// stay inside the reply. A reply with a wide table uses the transcript
    /// width, the transcript scrolls to the end of the table, and the wheel
    /// scrolls past the table. The table check runs with the Connections
    /// sidebar shown and hidden.
    func testAssistantLayout() throws {
        key(38, flags: .maskCommand) // Cmd+J opens the docked assistant.
        _ = try wait("Model: Synthetic Model", timeout: 20)
        if find("Search Conversations") != nil {
            try press("Toggle Conversation List")
            try waitGone("Search Conversations", timeout: 5)
        }
        try checkInlineCodeMessage()
        try checkBoldReply()
        try checkComposerPadding()
        // Earlier messages make the transcript long enough to scroll.
        try fill("Assistant Message", "Show many lines")
        try press("Send")
        _ = try wait("Line 40", timeout: 20)
        try fill("Assistant Message", "Show a wide table")
        try press("Send")
        _ = try wait("Assistant: Ran a synthetic wide table query", timeout: 20)
        try checkWideTableReply("assistant-wide-table")
        key(11, flags: .maskCommand) // Cmd+B hides the Connections sidebar.
        try waitGone("New Connection", timeout: 5)
        try checkWideTableReply("assistant-wide-table-no-sidebar")
        print("PASS: Assistant messages with inline code show every line, bold text wraps inside the reply, and wide table replies use the transcript width, show the whole table, and scroll")
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
            _ = try command(["screencapture", "-x", "-R", "\(Int(origin.x)),\(Int(origin.y)),\(Int(extent.width)),\(Int(extent.height))", path])
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
    /// At this width the message fits on one line. If the text breaks before
    /// the last word, the bubble keeps its one-line height and hides the
    /// second line, and the message extends below the bubble. Left of the
    /// text, the bottom of the message must have the bubble color of its top.
    func checkInlineCodeMessage() throws {
        try fill("Assistant Message", "How many tables are in `sandbox_vbazhan` schema?")
        try press("Send")
        _ = try wait("I can help with this query", timeout: 20)
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.5))
        let message = try wait("You: How many tables are in", timeout: 5)
        let (origin, extent) = try elementBounds(message)
        try snapshot("assistant-inline-code-message")
        let path = "\(artifacts)/assistant-inline-code-message-edge.png"
        let rect = "\(Int(origin.x) + 1),\(Int(origin.y)),1,\(Int(extent.height))"
        _ = try command(["screencapture", "-x", "-R", rect, path])
        guard let data = FileManager.default.contents(atPath: path),
              let bitmap = NSBitmapImageRep(data: data),
              let top = bitmap.colorAt(x: 0, y: 2)?.usingColorSpace(.deviceRGB),
              let bottom = bitmap.colorAt(x: 0, y: bitmap.pixelsHigh - 3)?.usingColorSpace(.deviceRGB) else {
            throw Failure("Could not read the message edge capture")
        }
        let difference = max(abs(top.redComponent - bottom.redComponent),
                             abs(top.greenComponent - bottom.greenComponent),
                             abs(top.blueComponent - bottom.blueComponent))
        try require(difference < 0.02, "The message with inline code extends below its bubble")
        try checkMessageInsets(message)
    }
    /// A line that is wider than the reply is clipped at the reply's right
    /// edge, so glyphs touch that edge. A line that wraps correctly ends
    /// before it. The reply is drawn on the transcript background.
    func checkBoldReply() throws {
        try fill("Assistant Message", "Show a bold reply")
        try press("Send")
        let reply = try wait("Assistant: There were **44,266,382 distinct searches**", timeout: 20)
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.5))
        let (origin, extent) = try elementBounds(reply)
        let (composerOrigin, _) = try elementBounds(try waitInput("Assistant Message"))
        try snapshot("assistant-bold-reply")
        let bottom = min(origin.y + extent.height, composerOrigin.y)
        try require(bottom - origin.y > 100, "The bold reply is not on screen")
        let path = "\(artifacts)/assistant-bold-reply-edge.png"
        let rect = "\(Int(origin.x - 4)),\(Int(origin.y)),\(Int(extent.width) + 4),\(Int(bottom - origin.y))"
        _ = try command(["screencapture", "-x", "-R", rect, path])
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
        let (sendOrigin, sendSize) = try elementBounds(try waitAny(["Send · Ask", "Send · Run"]))
        let top = Int(composerOrigin.y) - 24
        let height = Int(sendOrigin.y + sendSize.height) + 24 - top
        let path = "\(artifacts)/assistant-composer-padding.png"
        _ = try command(["screencapture", "-x", "-R", "\(Int(composerOrigin.x + composerSize.width) + 4),\(top),1,\(height)", path])
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
    func checkMessageInsets(_ message: AXUIElement) throws {
        let reply = try wait("Assistant: **I can help with this query", timeout: 5)
        let (composerOrigin, composerSize) = try elementBounds(try waitInput("Assistant Message"))
        let (replyOrigin, _) = try elementBounds(reply)
        try require(abs(replyOrigin.x - composerOrigin.x) <= 1,
                    "The assistant reply starts \(replyOrigin.x - composerOrigin.x) points right of the composer")

        // Scan a row through the user bubble from the transcript background
        // on its left, and find the last point that has another color.
        let (origin, extent) = try elementBounds(message)
        let left = Int(composerOrigin.x) + 2
        let width = Int(composerOrigin.x + composerSize.width) + 6 - left
        let path = "\(artifacts)/assistant-message-insets.png"
        _ = try command(["screencapture", "-x", "-R", "\(left),\(Int(origin.y + extent.height / 2)),\(width),1", path])
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
    func checkWideTableReply(_ name: String) throws {
        // The tooltip has the same text, so look for the button only.
        func jumpButtonVisible() -> Bool { find("Jump to Latest", role: kAXButtonRole) != nil }
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.5))
        if jumpButtonVisible() { try press("Jump to Latest") }
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.5))
        try require(!jumpButtonVisible(), "The transcript did not scroll to the latest message")
        let reply = try wait("Assistant: Ran a synthetic wide table query", timeout: 5)
        let composer = try waitInput("Assistant Message")
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
    func openAssistantRunModeConfirmation() throws {
        let send = try wait("Send · Ask", role: kAXButtonRole)
        let (sendOrigin, sendSize) = try elementBounds(send)
        // GPUI Kit exposes the split button's caret as an unnamed AX button.
        // Select the adjacent caret by its bounds, then choose its named item.
        let caret = elements().first { element in
            guard attribute(element, kAXRoleAttribute) as? String == kAXButtonRole,
                  strings(element).isEmpty,
                  let (origin, size) = try? elementBounds(element) else { return false }
            return abs(origin.y - sendOrigin.y) < 2
                && origin.x >= sendOrigin.x + sendSize.width - 2
                && size.width > 0 && size.width < 60
        }
        guard let caret else { throw Failure("Assistant mode caret was not found") }
        try click(caret)
        try activate(try waitExact("Run automatically", timeout: 10, role: kAXMenuItemRole))
        _ = try waitExact("Run automatically", timeout: 10, role: kAXButtonRole)
    }
    func setAssistantRunMode() throws {
        try openAssistantRunModeConfirmation()
        try press("Cancel")
        _ = try wait("Send · Ask", timeout: 10, role: kAXButtonRole)
        try openAssistantRunModeConfirmation()
        try press("Run automatically")
        _ = try wait("Send · Run", timeout: 10, role: kAXButtonRole)
    }
    func testAssistantQueries() throws {
        try fill("SQL Editor", "SELECT 1 AS assistant_value")
        key(38, flags: .maskCommand) // Reopen the existing conversation.
        _ = try wait("Send · Ask", timeout: 20)
        try fill("Assistant Message", "Run selected SQL with approval")
        try press("Send")
        _ = try wait("Run in ", timeout: 20)
        try require(find("Assistant is working") == nil, "Working stayed visible while the query waited for approval")
        let cancel = try waitExact("Cancel Assistant Turn", timeout: 5, role: kAXButtonRole)
        try require(find("Send", role: kAXButtonRole) == nil, "Send remained visible during the assistant turn")
        let (cancelPosition, _) = try elementBounds(cancel)
        let (composerPosition, composerSize) = try elementBounds(try waitInput("Assistant Message"))
        try require(cancelPosition.y >= composerPosition.y + composerSize.height, "Cancel was not below the message field")
        let runButtons = elements().filter {
            attribute($0, kAXRoleAttribute) as? String == kAXButtonRole && strings($0).contains("Run")
        }
        try require(runButtons.count == 2, "Expected toolbar Run and assistant approval Run")
        try activate(runButtons[1])
        _ = try wait("I ran the query.", timeout: 90)
        _ = try wait("Send", timeout: 5, role: kAXButtonRole)
        _ = try wait("1", role: kAXCellRole)

        try setAssistantRunMode()
        try fill("SQL Editor", "SELECT 2 AS assistant_value")
        try fill("Assistant Message", "Run selected SQL automatically")
        try press("Send")
        _ = try wait("2", role: kAXCellRole)
        try require(find("Run in ") == nil, "Automatic mode requested approval")
        // A turn that ends after the pane closes marks its tab as unread, and
        // later checks find the tab by its exact name.
        try waitGone("Assistant is working", timeout: 20)
        try press("Toggle Assistant")
        try waitGone("Toggle Conversation List")
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
        try enterSubmenu("Copy to Connection…", showing: "Qrow E2E copy")
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
        try enterSubmenu("Move to Connection…", showing: "Qrow E2E copy")
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
        try press("Logs Panel")
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
        try press("Logs Panel")
        _ = try wait("Disconnected", timeout: 10)
        try press("Results Panel")
        _ = try wait("switch-b-reconnected")
        try query("SELECT 'switch-b-after-disconnect' AS value")
        _ = try wait("switch-b-after-disconnect")
        try selectConnection("Qrow E2E")
        try contextMenu("Qrow E2E copy", exact: true, role: kAXButtonRole)
        try pressMenuItem("Delete")
        try press("Delete connection")
        try waitGone("Qrow E2E copy")
        try press("Logs Panel")
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
        key(12, flags: .maskCommand) // Cmd+Q asks before stopping a running query.
        _ = try wait("Keep Working", timeout: 10, role: kAXButtonRole)
        try require(process.isRunning, "Quit closed Qrow while a query was running")
        try press("Keep Working")
        try waitGone("Keep Working", timeout: 5)
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
        try testActivityRetention()
        try testFailedSaveExit(closeWindow: false)
        print("PASS: About dialog, Settings dialog, connection menus, connection validation, unique connection names, tab duplication, unique tab names, tab rename, connection form, connection switching, retained results, real results, pagination, Unicode selection, concurrent tabs, server cancellation, reconnect, activity retention")
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
            } else if CommandLine.arguments.contains("--assistant-append-only") {
                try driver.seedAssistantLayoutWorkspace()
                try driver.start()
                try driver.testAssistantAppend()
            } else if CommandLine.arguments.contains("--assistant-titles-only") {
                try driver.seedAssistantLayoutWorkspace()
                try driver.start()
                try driver.testAssistantConversationTitles()
            } else if CommandLine.arguments.contains("--assistant-statement-only") {
                try driver.seedAssistantStatementWorkspace()
                try driver.start()
                try driver.testAssistantStatementRun()
            } else if CommandLine.arguments.contains("--assistant-retarget-only") {
                try driver.seedAssistantRetargetWorkspace()
                try driver.start()
                try driver.testAssistantRetargetAfterRename()
            } else if CommandLine.arguments.contains("--assistant-wrong-tab-only") {
                try driver.seedAssistantConnectionsWorkspace(alphaSQL: "SELECT 1;")
                try driver.start()
                try driver.testAssistantWrongActionTab()
            } else if CommandLine.arguments.contains("--assistant-parallel-only") {
                try driver.seedAssistantConnectionsWorkspace(alphaSQL: "")
                try driver.start()
                try driver.testAssistantParallel()
            } else if CommandLine.arguments.contains("--assistant-tab-title-only") {
                try driver.seedAssistantConnectionsWorkspace(alphaSQL: "")
                try driver.start()
                try driver.testAssistantTabTitles()
            } else if CommandLine.arguments.contains("--assistant-tab-binding-only") {
                try driver.seedAssistantConnectionsWorkspace(alphaSQL: "SELECT 5;")
                try driver.start()
                try driver.testAssistantTabBinding()
            } else if CommandLine.arguments.contains("--assistant-thread-list-only") {
                // A 1024-point hosted runner display fits a wide pane only below 1.0 scale.
                try driver.seedAssistantLayoutWorkspace(panelWidth: 900, uiScale: 0.9)
                try driver.start()
                try driver.testAssistantThreadList()
            } else if CommandLine.arguments.contains("--assistant-sign-in-only") {
                try driver.seedAssistantSignInWorkspace()
                try driver.start()
                try driver.testAssistantSignIn()
            } else if CommandLine.arguments.contains("--assistant-font-only") {
                try driver.start()
                try driver.testAssistant(fontOnly: true)
            } else if CommandLine.arguments.contains("--assistant-layout-only") {
                try driver.seedAssistantLayoutWorkspace()
                try driver.start()
                try driver.testAssistantLayout()
            } else if CommandLine.arguments.contains("--assistant-selection-only") {
                try driver.seedAssistantLayoutWorkspace(theme: "One Dark")
                try driver.start()
                try driver.testAssistantSelection()
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
            || CommandLine.arguments.contains("--assistant-layout-only")
            || CommandLine.arguments.contains("--assistant-selection-only")
            || CommandLine.arguments.contains("--assistant-append-only")
            || CommandLine.arguments.contains("--assistant-titles-only")
            || CommandLine.arguments.contains("--assistant-statement-only")
            || CommandLine.arguments.contains("--assistant-retarget-only")
            || CommandLine.arguments.contains("--assistant-wrong-tab-only")
            || CommandLine.arguments.contains("--assistant-sign-in-only")
            || CommandLine.arguments.contains("--assistant-parallel-only")
            || CommandLine.arguments.contains("--assistant-tab-title-only")
            || CommandLine.arguments.contains("--assistant-tab-binding-only")
            || CommandLine.arguments.contains("--assistant-thread-list-only") { exit(0) }
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
