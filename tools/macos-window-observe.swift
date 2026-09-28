#!/usr/bin/env swift
import CoreGraphics
import Foundation
import Vision

// External acceptance observer: no runtime JNI/View-tree diagnostics.
func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data("macos-window-observe: \(message)\n".utf8))
    exit(1)
}
let args = CommandLine.arguments
guard (args.count == 3 || (args.count == 4 && args[3] == "top")), let pid = Int32(args[1]) else {
    fail("usage: macos-window-observe.swift PID OUTPUT.png [top]")
}
let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements],
                                        kCGNullWindowID) as? [[String: Any]] ?? []
guard let window = windows.first(where: {
    ($0[kCGWindowOwnerPID as String] as? Int32) == pid &&
    ($0[kCGWindowLayer as String] as? Int) == 0
}), let id = window[kCGWindowNumber as String] as? UInt32 else {
    fail("no visible application window for pid \(pid)")
}
let capture = Process()
capture.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
capture.arguments = ["-x", "-l", String(id), args[2]]
try capture.run()
capture.waitUntilExit()
guard capture.terminationStatus == 0 else { fail("window capture failed") }
let request = VNRecognizeTextRequest()
request.recognitionLevel = .accurate
request.usesLanguageCorrection = false
request.recognitionLanguages = ["en-US"]
try VNImageRequestHandler(url: URL(fileURLWithPath: args[2])).perform([request])
for observation in request.results ?? [] {
    if args.count == 4 && observation.boundingBox.minY < 0.65 { continue }
    if let candidate = observation.topCandidates(1).first { print(candidate.string) }
}
