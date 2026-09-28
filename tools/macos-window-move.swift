#!/usr/bin/env swift
// Moves the exact pid's layer-zero window by a physical title-bar drag.
import AppKit
import CoreGraphics

private func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data("macos-window-move: \(message)\n".utf8))
    exit(64)
}

let arguments = CommandLine.arguments
guard arguments.count == 4, let pid = Int32(arguments[1]),
      let dx = Double(arguments[2]), let dy = Double(arguments[3]) else {
    fail("usage: macos-window-move.swift PID DX DY")
}
guard CGPreflightPostEventAccess() else { fail("Accessibility permission is required") }
let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements],
                                         kCGNullWindowID) as? [[String: Any]] ?? []
guard let window = windows.first(where: {
    ($0[kCGWindowOwnerPID as String] as? Int32) == pid &&
        ($0[kCGWindowLayer as String] as? Int) == 0
}), let bounds = window[kCGWindowBounds as String] as? [String: Double] else {
    fail("no visible layer-zero window for pid \(pid)")
}
_ = NSRunningApplication(processIdentifier: pid)?.activate(options: [.activateAllWindows])
usleep(300_000)
let source = CGEventSource(stateID: .hidSystemState)
let start = CGPoint(x: bounds["X"]! + bounds["Width"]! / 2 + 60, y: bounds["Y"]! + 12)
func post(_ type: CGEventType, _ point: CGPoint) {
    CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: point,
            mouseButton: .left)?.post(tap: .cghidEventTap)
    usleep(40_000)
}
post(.mouseMoved, start)
post(.leftMouseDown, start)
for step in 1...10 {
    let fraction = Double(step) / 10
    post(.leftMouseDragged, CGPoint(x: start.x + dx * fraction, y: start.y + dy * fraction))
}
post(.leftMouseUp, CGPoint(x: start.x + dx, y: start.y + dy))
print("macos-window-move: pid=\(pid) dx=\(dx) dy=\(dy)")
