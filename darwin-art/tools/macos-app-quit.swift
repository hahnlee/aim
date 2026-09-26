#!/usr/bin/env swift
// Asks the exact pid's application to quit with the Apple Event the Dock and
// the app menu use (ADR 0011: Quit removes the Android task).
import Carbon
import Foundation

private func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data("macos-app-quit: \(message)\n".utf8))
    exit(64)
}

let arguments = CommandLine.arguments
guard arguments.count == 2, let pid = Int32(arguments[1]), pid > 0 else {
    fail("usage: macos-app-quit.swift PID")
}
let event = NSAppleEventDescriptor(
    eventClass: AEEventClass(kCoreEventClass), eventID: AEEventID(kAEQuitApplication),
    targetDescriptor: NSAppleEventDescriptor(processIdentifier: pid),
    returnID: AEReturnID(kAutoGenerateReturnID), transactionID: AETransactionID(kAnyTransactionID))
do {
    // The app replies only once it has terminated, so do not wait.
    _ = try event.sendEvent(options: [.noReply], timeout: 5)
} catch {
    fail("pid \(pid): \(error)")
}
