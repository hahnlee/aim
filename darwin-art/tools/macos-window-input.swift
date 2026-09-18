#!/usr/bin/env swift

import CoreGraphics
import Foundation
import AppKit

private func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data("macos-window-input: \(message)\n".utf8))
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

private func click(_ point: CGPoint) {
    let source = CGEventSource(stateID: .hidSystemState)
    CGEvent(mouseEventSource: source, mouseType: .mouseMoved,
            mouseCursorPosition: point, mouseButton: .left)?.post(tap: .cghidEventTap)
    usleep(30_000)
    CGEvent(mouseEventSource: source, mouseType: .leftMouseDown,
            mouseCursorPosition: point, mouseButton: .left)?.post(tap: .cghidEventTap)
    usleep(50_000)
    CGEvent(mouseEventSource: source, mouseType: .leftMouseUp,
            mouseCursorPosition: point, mouseButton: .left)?.post(tap: .cghidEventTap)
}

let arguments = CommandLine.arguments
let mode = arguments.count > 1 ? arguments[1] : ""
guard arguments.count >= 4,
      mode == "click-content" || mode == "click-content-no-focus",
      let ownerPID = Int32(arguments[2]),
      let contentHeight = Double(arguments[3]), contentHeight > 0 else {
    fail("usage: macos-window-input.swift {click-content|click-content-no-focus} PID CONTENT_HEIGHT X,Y,DELAY_MS ...")
}

let bounds = waitForWindow(ownerPID: ownerPID, timeoutSeconds: 15)
let contentOrigin = CGPoint(x: bounds.minX, y: bounds.maxY - contentHeight)

if mode == "click-content" {
    // A title-bar click cannot select a target hidden behind another app at
    // the same position. Raise the requested macOS process before HID input;
    // this does not inject Android focus or call the guest input dispatcher.
    guard let application = NSRunningApplication(processIdentifier: ownerPID),
          application.activate(options: []) else {
        fail("could not activate macOS process \(ownerPID)")
    }
    usleep(150_000)
    // A real title-bar click activates and orders the target window before input.
    // Content coordinates remain Android logical pixels/macOS points; AppKit's
    // backing-scale conversion maps them to the Retina Android surface exactly once.
    click(CGPoint(x: bounds.midX, y: bounds.minY + 12))
    usleep(250_000)
}

for encoded in arguments.dropFirst(4) {
    let fields = encoded.split(separator: ",", omittingEmptySubsequences: false)
    guard fields.count == 3,
          let x = Double(fields[0]),
          let y = Double(fields[1]),
          let delayMilliseconds = UInt32(fields[2]) else {
        fail("invalid click X,Y,DELAY_MS: \(encoded)")
    }
    click(CGPoint(x: contentOrigin.x + x, y: contentOrigin.y + y))
    usleep(delayMilliseconds * 1_000)
}
