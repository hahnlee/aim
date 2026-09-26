#pragma once

#import <AppKit/AppKit.h>

namespace darwin_art::window {

// The caller supplies an already-resolved UTF-8 title. A null, empty, or
// malformed title uses the generic native-surface title; Android resource and
// package-name lookup remain outside this macOS provider.
NSString* DecodeSurfaceWindowTitle(const char* resolved_utf8_title);

// Projects an optional launcher icon supplied by the caller through
// DARWIN_ART_APK_APP_ICON onto the native application and window. Missing or
// malformed icon data leaves both existing AppKit icon properties unchanged.
// Image representations retain their native pixel dimensions while the image
// is given the conventional 128-point AppKit display size.
void ApplySurfaceApplicationIdentity(NSApplication* application,
                                     NSWindow* window);

// Installs the standard macOS main menu for an Android desktop window once per
// process: the application menu (hide items) and View > Enter Full Screen
// (Cmd+Ctrl+F). Quitting is not offered here: Cmd+Q has no Android lifecycle
// yet (#15).
void InstallSurfaceApplicationMenu(NSApplication* application, NSString* title);

}  // namespace darwin_art::window
