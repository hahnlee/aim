#!/usr/bin/env swift
// Brings the exact pid's application back the way launching a running macOS
// app does: activate it and send the reopen Apple Event (ADR 0011: a hidden
// window is shown and its task resumes). Exits 3 when the pid is not a
// regular windowed application, such as a service-only process.
import AppKit
import Carbon

private func fail(_ message: String, _ status: Int32 = 64) -> Never {
    FileHandle.standardError.write(Data("macos-app-reopen: \(message)\n".utf8))
    exit(status)
}

let arguments = CommandLine.arguments
guard arguments.count == 2, let pid = Int32(arguments[1]), pid > 0 else {
    fail("usage: macos-app-reopen.swift PID")
}
guard let application = NSRunningApplication(processIdentifier: pid),
      application.activationPolicy == .regular else {
    fail("pid \(pid) is not a regular application", 3)
}
let event = NSAppleEventDescriptor(
    eventClass: AEEventClass(kCoreEventClass), eventID: AEEventID(kAEReopenApplication),
    targetDescriptor: NSAppleEventDescriptor(processIdentifier: pid),
    returnID: AEReturnID(kAutoGenerateReturnID), transactionID: AETransactionID(kAnyTransactionID))
do {
    _ = try event.sendEvent(options: [.waitForReply], timeout: 5)
} catch {
    fail("pid \(pid): \(error)", 69)
}
_ = application.activate()
