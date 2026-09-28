#!/usr/bin/env swift

import CoreGraphics
import Foundation
import AppKit

private struct VisibleWindow {
    let ownerPID: Int32
    let bounds: CGRect
}

private func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data("macos-window-keyboard: \(message)\n".utf8))
    exit(64)
}

private func visibleLayerZeroWindows() -> [VisibleWindow] {
    let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
    guard let windows = CGWindowListCopyWindowInfo(options, kCGNullWindowID)
        as? [[String: Any]] else {
        return []
    }
    return windows.compactMap { window in
        guard let owner = window[kCGWindowOwnerPID as String] as? NSNumber,
              let layer = window[kCGWindowLayer as String] as? NSNumber,
              layer.intValue == 0,
              let dictionary = window[kCGWindowBounds as String] as? [String: Any],
              let x = dictionary["X"] as? NSNumber,
              let y = dictionary["Y"] as? NSNumber,
              let width = dictionary["Width"] as? NSNumber,
              let height = dictionary["Height"] as? NSNumber else {
            return nil
        }
        if let alpha = window[kCGWindowAlpha as String] as? NSNumber, alpha.doubleValue <= 0 {
            return nil
        }
        let bounds = CGRect(x: x.doubleValue, y: y.doubleValue,
                            width: width.doubleValue, height: height.doubleValue)
        guard bounds.width > 0, bounds.height > 0 else { return nil }
        return VisibleWindow(ownerPID: owner.int32Value, bounds: bounds)
    }
}

private func windowBounds(ownerPID: Int32) -> CGRect? {
    visibleLayerZeroWindows().first(where: { $0.ownerPID == ownerPID })?.bounds
}

private func foregroundPID() -> Int32? {
    // NSWorkspace refreshes frontmostApplication from its notification/run-loop
    // state. Pump briefly so a preceding physical title-bar click is observed
    // before deciding where the next HID sequence would be delivered.
    RunLoop.current.run(until: Date().addingTimeInterval(0.01))
    return NSWorkspace.shared.frontmostApplication?.processIdentifier
}

// CGWindowListCopyWindowInfo is ordered front-to-back. The first visible
// layer-zero window containing the event point is the recipient of a mouse
// event posted at that point.
private func frontmostWindow(at point: CGPoint) -> VisibleWindow? {
    visibleLayerZeroWindows().first(where: { $0.bounds.contains(point) })
}

private func waitForWindow(ownerPID: Int32, timeoutSeconds: Double) -> CGRect {
    let deadline = Date().addingTimeInterval(timeoutSeconds)
    repeat {
        if let bounds = windowBounds(ownerPID: ownerPID) {
            return bounds
        }
        usleep(50_000)
    } while Date() < deadline
    fail("no on-screen layer-zero window for pid \(ownerPID)")
}

private func waitForTarget(ownerPID: Int32,
                           pointForBounds: (CGRect) -> CGPoint,
                           purpose: String,
                           timeoutSeconds: Double,
                           requireForeground: Bool = true) -> CGRect {
    let deadline = Date().addingTimeInterval(timeoutSeconds)
    var reason = "no target window"
    repeat {
        if let bounds = windowBounds(ownerPID: ownerPID) {
            let point = pointForBounds(bounds)
            let foreground = foregroundPID()
            let frontmost = frontmostWindow(at: point)
            if frontmost?.ownerPID == ownerPID && (!requireForeground || foreground == ownerPID) {
                return bounds
            }
            let foregroundDescription = foreground.map(String.init) ?? "none"
            let frontmostDescription = frontmost.map { String($0.ownerPID) } ?? "none"
            reason = "foreground pid=\(foregroundDescription), frontmost layer-zero pid=\(frontmostDescription) at (\(point.x),\(point.y))"
        }
        usleep(50_000)
    } while Date() < deadline
    let requirement = requireForeground ? "foreground and frontmost" : "frontmost at the event point"
    fail("pid \(ownerPID) did not become the exact \(requirement) for \(purpose) within \(timeoutSeconds)s (\(reason)); refusing to post HID input")
}

private func requireTarget(ownerPID: Int32, bounds: CGRect, point: CGPoint,
                           purpose: String, requireForeground: Bool = true) {
    let foreground = foregroundPID()
    guard !requireForeground || foreground == ownerPID else {
        let foregroundDescription = foreground.map(String.init) ?? "none"
        fail("pid \(ownerPID) is not the foreground application for \(purpose) at (\(point.x),\(point.y)); foreground pid=\(foregroundDescription), refusing to post HID input")
    }
    guard bounds.contains(point) else {
        fail("\(purpose) point (\(point.x),\(point.y)) is outside target pid \(ownerPID) bounds \(bounds); refusing to post HID input")
    }
    guard let frontmost = frontmostWindow(at: point) else {
        fail("no visible layer-zero window covers the \(purpose) point (\(point.x),\(point.y)) for target pid \(ownerPID); refusing to post HID input")
    }
    guard frontmost.ownerPID == ownerPID else {
        fail("\(purpose) point (\(point.x),\(point.y)) is frontmost-owned by pid \(frontmost.ownerPID), expected target pid \(ownerPID); refusing to post HID input")
    }
}

private func postTitlebarActivation(_ point: CGPoint) {
    let source = CGEventSource(stateID: .hidSystemState)
    guard let moved = CGEvent(mouseEventSource: source, mouseType: .mouseMoved,
                              mouseCursorPosition: point, mouseButton: .left),
          let down = CGEvent(mouseEventSource: source, mouseType: .leftMouseDown,
                             mouseCursorPosition: point, mouseButton: .left),
          let up = CGEvent(mouseEventSource: source, mouseType: .leftMouseUp,
                           mouseCursorPosition: point, mouseButton: .left) else {
        fail("could not create title-bar activation events")
    }
    moved.post(tap: .cghidEventTap)
    usleep(30_000)
    down.post(tap: .cghidEventTap)
    usleep(50_000)
    up.post(tap: .cghidEventTap)
}

private func postKey(_ keyCode: UInt16, flags: CGEventFlags) {
    let source = CGEventSource(stateID: .hidSystemState)
    guard let down = CGEvent(keyboardEventSource: source, virtualKey: keyCode,
                             keyDown: true),
          let up = CGEvent(keyboardEventSource: source, virtualKey: keyCode,
                           keyDown: false) else {
        fail("could not create keyboard events for keycode \(keyCode)")
    }
    down.flags = flags
    up.flags = flags
    down.post(tap: .cghidEventTap)
    usleep(30_000)
    up.post(tap: .cghidEventTap)
}

private func parseKeySpec(_ encoded: Substring) -> (UInt16, CGEventFlags) {
    let spec = encoded.split(separator: ":", omittingEmptySubsequences: false)
    guard !spec.isEmpty, spec.count <= 2, !spec[0].isEmpty,
          let keyCodeValue = UInt32(spec[0]), keyCodeValue <= 127 else {
        fail("invalid keycode or modifier syntax (keycode 0...127): \(encoded)")
    }
    var flags: CGEventFlags = []
    guard spec.count == 2 else { return (UInt16(keyCodeValue), flags) }
    guard !spec[1].isEmpty else {
        fail("empty modifier list in key specification: \(encoded)")
    }
    var seen = Set<String>()
    for modifier in spec[1].split(separator: "+", omittingEmptySubsequences: false) {
        let name = String(modifier)
        guard !name.isEmpty else {
            fail("empty modifier in key specification: \(encoded)")
        }
        guard seen.insert(name).inserted else {
            fail("duplicate modifier \(name) in key specification: \(encoded)")
        }
        switch name {
        case "shift": flags.insert(.maskShift)
        case "command": flags.insert(.maskCommand)
        case "option": flags.insert(.maskAlternate)
        case "control": flags.insert(.maskControl)
        default: fail("unknown modifier \(name) in key specification: \(encoded)")
        }
    }
    return (UInt16(keyCodeValue), flags)
}

let arguments = CommandLine.arguments
guard arguments.count >= 4,
      let ownerPID = Int32(arguments[1]), ownerPID > 0,
      let contentHeight = Double(arguments[2]), contentHeight > 0 else {
    fail("usage: macos-window-keyboard.swift PID CONTENT_HEIGHT KEYCODE[:MODIFIER+...],DELAY_MS ...")
}

guard CGPreflightPostEventAccess() else {
    fail("Accessibility permission is required to post CGEvents")
}

let initialBounds = waitForWindow(ownerPID: ownerPID, timeoutSeconds: 15)
// Window-list bounds and CONTENT_HEIGHT are both macOS points.  Validate the
// title-bar geometry explicitly; keyboard events carry virtual key codes and
// do not apply an Android/content-to-Retina coordinate transform.
let titlebarHeight = initialBounds.height - contentHeight
guard titlebarHeight > 0, titlebarHeight < initialBounds.height else {
    fail("content height \(contentHeight) does not leave a title bar in \(initialBounds)")
}

var keys: [(UInt16, CGEventFlags, UInt32)] = []
for encoded in arguments.dropFirst(3) {
    let fields = encoded.split(separator: ",", omittingEmptySubsequences: false)
    guard fields.count == 2,
          let delayMilliseconds = UInt32(fields[1]),
          delayMilliseconds <= UInt32.max / 1_000 else {
        fail("invalid keycode[:modifier+...],delay-ms: \(encoded)")
    }
    let (keyCode, flags) = parseKeySpec(fields[0])
    keys.append((keyCode, flags, delayMilliseconds))
}

// activateAllWindows is the documented AppKit operation for ordering every
// target window. It may report success while another app remains foreground,
// so the exact title-bar owner is checked before posting the switch click.
guard let application = NSRunningApplication(processIdentifier: ownerPID),
      application.activate(options: [.activateAllWindows]) else {
    fail("could not activate macOS process \(ownerPID)")
}
var bounds = waitForTarget(
    ownerPID: ownerPID,
    pointForBounds: {
        let currentTitlebarHeight = $0.height - contentHeight
        return CGPoint(x: $0.midX,
                       y: $0.minY + min(12, currentTitlebarHeight / 2))
    },
    purpose: "title-bar activation",
    timeoutSeconds: 5,
    requireForeground: false)
let refreshedTitlebarHeight = bounds.height - contentHeight
guard refreshedTitlebarHeight > 0, refreshedTitlebarHeight < bounds.height else {
    fail("content height \(contentHeight) does not leave a title bar in refreshed bounds \(bounds)")
}
let titlebarPoint = CGPoint(x: bounds.midX,
                            y: bounds.minY + min(12, refreshedTitlebarHeight / 2))
requireTarget(ownerPID: ownerPID, bounds: bounds, point: titlebarPoint,
              purpose: "title-bar activation", requireForeground: false)
// The title-bar click is the physical mechanism that may switch a visible
// side-by-side target; no Android focus or guest input dispatch is injected.
postTitlebarActivation(titlebarPoint)
usleep(250_000)

// Refresh geometry after activation and verify exact foreground/frontmost
// ownership before posting the keyboard HID sequence.
bounds = waitForTarget(
    ownerPID: ownerPID,
    pointForBounds: { CGPoint(x: $0.midX, y: $0.minY + min(12, ($0.height - contentHeight) / 2)) },
    purpose: "keyboard input",
    timeoutSeconds: 2)

for (keyCode, flags, delayMilliseconds) in keys {
    guard let keyBounds = windowBounds(ownerPID: ownerPID) else {
        fail("target pid \(ownerPID) lost its on-screen layer-zero window before keycode \(keyCode)")
    }
    let keyTitlebarHeight = keyBounds.height - contentHeight
    guard keyTitlebarHeight > 0 else {
        fail("target pid \(ownerPID) no longer has a title bar before keycode \(keyCode)")
    }
    let keyEventPoint = CGPoint(x: keyBounds.midX,
                                y: keyBounds.minY + min(12, keyTitlebarHeight / 2))
    requireTarget(ownerPID: ownerPID, bounds: keyBounds, point: keyEventPoint,
                  purpose: "keyboard keycode \(keyCode)")
    postKey(keyCode, flags: flags)
    usleep(delayMilliseconds * 1_000)
}

print("macos-window-keyboard: PASS pid=\(ownerPID) key-events=\(keys.count)")
