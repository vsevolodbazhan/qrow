// Native window observation does not require Accessibility or screen recording.
import CoreGraphics
import Foundation

guard CommandLine.arguments.count == 2,
      let pid = Int32(CommandLine.arguments[1]) else { exit(2) }
let windows = CGWindowListCopyWindowInfo(
    [.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID
) as? [[String: Any]] ?? []
let visible = windows.contains { window in
    guard let owner = window[kCGWindowOwnerPID as String] as? NSNumber,
          owner.int32Value == pid,
          let layer = window[kCGWindowLayer as String] as? NSNumber,
          layer.intValue == 0,
          let bounds = window[kCGWindowBounds as String] as? [String: Any],
          let rectangle = CGRect(dictionaryRepresentation: bounds as CFDictionary) else { return false }
    return rectangle.width > 0 && rectangle.height > 0
}
exit(visible ? 0 : 1)
