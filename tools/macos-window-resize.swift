#!/usr/bin/env swift

import AppKit
import CoreGraphics
import Foundation

private let tolerance: CGFloat = 4

private func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data("macos-window-resize: \(message)\n".utf8))
    exit(64)
}

private func windowBounds(ownerPID: Int32) -> CGRect? {
    let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
    guard let windows = CGWindowListCopyWindowInfo(options, kCGNullWindowID)
        as? [[String: Any]] else {
        return nil
    }

    for window in windows {
        guard let pid = (window[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value,
              pid == ownerPID,
              (window[kCGWindowLayer as String] as? NSNumber)?.intValue == 0,
              let dictionary = window[kCGWindowBounds as String] as? [String: Any],
              let x = (dictionary["X"] as? NSNumber)?.doubleValue,
              let y = (dictionary["Y"] as? NSNumber)?.doubleValue,
              let width = (dictionary["Width"] as? NSNumber)?.doubleValue,
              let height = (dictionary["Height"] as? NSNumber)?.doubleValue else {
            continue
        }
        let bounds = CGRect(x: x, y: y, width: width, height: height)
        if bounds.origin.x.isFinite, bounds.origin.y.isFinite,
           bounds.width.isFinite, bounds.height.isFinite,
           bounds.width > 0, bounds.height > 0 {
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
    fail("missing on-screen layer-zero window for exact pid \(ownerPID)")
}

private func waitForResize(ownerPID: Int32, width: CGFloat, height: CGFloat,
                           timeoutSeconds: Double) -> CGRect {
    let deadline = Date().addingTimeInterval(timeoutSeconds)
    var latest: CGRect?
    repeat {
        if let bounds = windowBounds(ownerPID: ownerPID) {
            latest = bounds
            if abs(bounds.width - width) <= tolerance,
               abs(bounds.height - height) <= tolerance {
                return bounds
            }
        }
        usleep(50_000)
    } while Date() < deadline

    guard let latest else {
        fail("missing on-screen layer-zero window for exact pid \(ownerPID) after resize")
    }
    return latest
}

private func postMouseEvent(source: CGEventSource, type: CGEventType,
                            point: CGPoint) {
    guard let event = CGEvent(mouseEventSource: source, mouseType: type,
                              mouseCursorPosition: point, mouseButton: .left) else {
        fail("could not create CGEvent at (\(point.x),\(point.y))")
    }
    event.post(tap: .cghidEventTap)
}

private func describe(_ bounds: CGRect) -> String {
    String(format: "x=%.1f y=%.1f width=%.1f height=%.1f",
           bounds.origin.x, bounds.origin.y, bounds.width, bounds.height)
}

let arguments = CommandLine.arguments
guard arguments.count == 4,
      let ownerPID = Int32(arguments[1]), ownerPID > 0,
      let targetWidth = Double(arguments[2]), targetWidth.isFinite, targetWidth > 0,
      let targetHeight = Double(arguments[3]), targetHeight.isFinite, targetHeight > 0 else {
    fail("usage: macos-window-resize.swift PID TARGET_OUTER_WIDTH TARGET_OUTER_HEIGHT")
}

guard CGPreflightPostEventAccess() else {
    fail("Accessibility permission is required to post CGEvents")
}

guard let application = NSRunningApplication(processIdentifier: ownerPID) else {
    fail("no running macOS application for exact pid \(ownerPID)")
}

let original = waitForWindow(ownerPID: ownerPID, timeoutSeconds: 15)
guard application.activate(options: []) else {
    fail("could not activate macOS process \(ownerPID)")
}
usleep(150_000)

guard let source = CGEventSource(stateID: .hidSystemState) else {
    fail("could not create HID event source")
}

let targetSize = CGSize(width: targetWidth, height: targetHeight)
// The corner point is deliberately inside the window. It is a physical
// AppKit/macOS-point gesture; no Android raster or guest API is involved.
let inset = min(4, max(1, min(original.width, original.height) / 2))
let startCorner = CGPoint(x: original.maxX - inset, y: original.maxY - inset)
let desiredCorner = CGPoint(x: original.minX + targetSize.width - inset,
                            y: original.minY + targetSize.height - inset)

postMouseEvent(source: source, type: .mouseMoved, point: startCorner)
usleep(50_000)
postMouseEvent(source: source, type: .leftMouseDown, point: startCorner)

// RESIZE_STEPS=1 produces a single drag event (one host resize fact).
let steps = max(1, Int(ProcessInfo.processInfo.environment["RESIZE_STEPS"] ?? "10") ?? 10)
for step in 1...steps {
    let fraction = CGFloat(step) / CGFloat(steps)
    let point = CGPoint(x: startCorner.x + (desiredCorner.x - startCorner.x) * fraction,
                        y: startCorner.y + (desiredCorner.y - startCorner.y) * fraction)
    postMouseEvent(source: source, type: .leftMouseDragged, point: point)
    usleep(50_000)
}
postMouseEvent(source: source, type: .leftMouseUp, point: desiredCorner)

let updated = waitForResize(ownerPID: ownerPID, width: targetSize.width,
                            height: targetSize.height, timeoutSeconds: 5)
print("macos-window-resize: pid=\(ownerPID) original={\(describe(original))} " +
      "new={\(describe(updated))} target={width=\(targetWidth) height=\(targetHeight)}")

guard abs(updated.width - targetSize.width) <= tolerance,
      abs(updated.height - targetSize.height) <= tolerance else {
    fail("resize differs from target by more than \(Int(tolerance)) macOS points")
}

print("macos-window-resize: PASS pid=\(ownerPID)")
