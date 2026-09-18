#!/usr/bin/env swift

import CoreGraphics
import Foundation
import AppKit

private func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data("macos-window-keyboard: \(message)\n".utf8))
    exit(64)
}

private func windowBounds(ownerPID: Int32) -> CGRect? {
    let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
    guard let windows = CGWindowListCopyWindowInfo(options, kCGNullWindowID)
        as? [[String: Any]] else {
        return nil
    }
    for window in windows {
        guard (window[kCGWindowOwnerPID as String] as? Int32) == ownerPID,
              (window[kCGWindowLayer as String] as? Int) == 0,
              let dictionary = window[kCGWindowBounds as String] as? [String: Any],
              let x = dictionary["X"] as? NSNumber,
              let y = dictionary["Y"] as? NSNumber,
              let width = dictionary["Width"] as? NSNumber,
              let height = dictionary["Height"] as? NSNumber else {
            continue
        }
        let bounds = CGRect(x: x.doubleValue, y: y.doubleValue,
                            width: width.doubleValue, height: height.doubleValue)
        if bounds.width > 0, bounds.height > 0 {
            return bounds
        }
    }
    return nil
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

let bounds = waitForWindow(ownerPID: ownerPID, timeoutSeconds: 15)
// Window-list bounds and CONTENT_HEIGHT are both macOS points.  Validate the
// title-bar geometry explicitly; keyboard events carry virtual key codes and
// do not apply an Android/content-to-Retina coordinate transform.
let titlebarHeight = bounds.height - contentHeight
guard titlebarHeight > 0, titlebarHeight < bounds.height else {
    fail("content height \(contentHeight) does not leave a title bar in \(bounds)")
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

// A real title-bar click activates and orders the target window before the
// HID keyboard events. The point is in the window-list/AppKit coordinate
// space; no Android surface coordinate is involved.
guard let application = NSRunningApplication(processIdentifier: ownerPID),
      application.activate(options: []) else {
    fail("could not activate macOS process \(ownerPID)")
}
usleep(150_000)
postTitlebarActivation(CGPoint(x: bounds.midX,
                               y: bounds.minY + min(12, titlebarHeight / 2)))
usleep(250_000)

for (keyCode, flags, delayMilliseconds) in keys {
    postKey(keyCode, flags: flags)
    usleep(delayMilliseconds * 1_000)
}

print("macos-window-keyboard: PASS pid=\(ownerPID) key-events=\(keys.count)")
