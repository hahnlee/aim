#!/usr/bin/env swift

import CoreGraphics
import Foundation
import AppKit

private struct VisibleWindow {
    let ownerPID: Int32
    let bounds: CGRect
}

private func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data("macos-window-input: \(message)\n".utf8))
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

if mode == "click-content" {
    _ = waitForWindow(ownerPID: ownerPID, timeoutSeconds: 15)
    // activateAllWindows is the documented AppKit operation for ordering every
    // target window. Activation can report success while another app remains
    // foreground, so the exact event-point owner is checked before posting.
    guard let application = NSRunningApplication(processIdentifier: ownerPID),
          application.activate(options: [.activateAllWindows]) else {
        fail("could not activate macOS process \(ownerPID)")
    }
    // A title-bar click is the physical mechanism that may switch a visible
    // side-by-side target. Permit another foreground PID before that click,
    // but require the target window to own the exact title-bar event point.
    let bounds = waitForTarget(
        ownerPID: ownerPID,
        pointForBounds: { CGPoint(x: $0.midX, y: $0.minY + 12) },
        purpose: "title-bar activation",
        timeoutSeconds: 5,
        requireForeground: false)
    let titlebarPoint = CGPoint(x: bounds.midX, y: bounds.minY + 12)
    guard bounds.contains(titlebarPoint) else {
        fail("title-bar activation point is outside target pid \(ownerPID) bounds \(bounds); refusing to post HID input")
    }
    click(titlebarPoint)
    usleep(250_000)
    // Geometry is refreshed per content event below. The exact event point is
    // checked immediately before each HID sequence.
} else {
    // Preserve no-focus semantics: do not activate or order the target. It
    // must already own the exact content event point or the tool fails safely.
    _ = waitForWindow(ownerPID: ownerPID, timeoutSeconds: 15)
}

for encoded in arguments.dropFirst(4) {
    let fields = encoded.split(separator: ",", omittingEmptySubsequences: false)
    guard fields.count == 3,
          let x = Double(fields[0]),
          let y = Double(fields[1]),
          let delayMilliseconds = UInt32(fields[2]) else {
        fail("invalid click X,Y,DELAY_MS: \(encoded)")
    }
    guard let bounds = windowBounds(ownerPID: ownerPID) else {
        fail("target pid \(ownerPID) lost its on-screen layer-zero window before content input")
    }
    let point = CGPoint(x: bounds.minX + x,
                        y: bounds.maxY - contentHeight + y)
    requireTarget(ownerPID: ownerPID, bounds: bounds, point: point,
                  purpose: "content input", requireForeground: mode == "click-content")
    click(point)
    usleep(delayMilliseconds * 1_000)
}
