#include "application_identity.h"

#include <cstdlib>

namespace darwin_art::window {
namespace {

NSString* GenericSurfaceTitle() {
  return @"Darwin ART Surface";
}

NSImage* ApplicationIconFromEnvironment() {
  const char* path = std::getenv("DARWIN_ART_APK_APP_ICON");
  if (path == nullptr || path[0] == '\0') return nil;
  NSString* file = [NSString stringWithUTF8String:path];
  if (file == nil) return nil;
  NSData* data = [NSData dataWithContentsOfFile:file options:0 error:nil];
  NSImage* image = data == nil ? nil : [[NSImage alloc] initWithData:data];
  if (image != nil) image.size = NSMakeSize(128.0, 128.0);
  return image;
}

}  // namespace

NSString* DecodeSurfaceWindowTitle(const char* resolved_utf8_title) {
  if (resolved_utf8_title == nullptr || resolved_utf8_title[0] == '\0')
    return GenericSurfaceTitle();
  NSString* title = [NSString stringWithUTF8String:resolved_utf8_title];
  return title == nil ? GenericSurfaceTitle() : title;
}

void ApplySurfaceApplicationIdentity(NSApplication* application,
                                     NSWindow* window) {
  NSImage* image = ApplicationIconFromEnvironment();
  if (image == nil) return;
  if (application != nil) application.applicationIconImage = image;
  if (window != nil) window.miniwindowImage = image;
}

void InstallSurfaceApplicationMenu(NSApplication* application, NSString* title) {
  if (application == nil || application.mainMenu != nil) return;
  NSString* name = title.length > 0 ? title : GenericSurfaceTitle();
  NSMenu* main = [[NSMenu alloc] initWithTitle:@""];

  NSMenuItem* app_item = [[NSMenuItem alloc] initWithTitle:name action:nil keyEquivalent:@""];
  NSMenu* app_menu = [[NSMenu alloc] initWithTitle:name];
  [app_menu addItemWithTitle:[@"Hide " stringByAppendingString:name]
                      action:@selector(hide:)
               keyEquivalent:@"h"];
  NSMenuItem* hide_others = [app_menu addItemWithTitle:@"Hide Others"
                                                action:@selector(hideOtherApplications:)
                                         keyEquivalent:@"h"];
  hide_others.keyEquivalentModifierMask =
      NSEventModifierFlagCommand | NSEventModifierFlagOption;
  [app_menu addItemWithTitle:@"Show All"
                      action:@selector(unhideAllApplications:)
               keyEquivalent:@""];
  app_item.submenu = app_menu;
  [main addItem:app_item];

  // toggleFullScreen: reaches the key window, whose resize becomes an Android
  // task geometry revision like any other root resize.
  NSMenuItem* view_item = [[NSMenuItem alloc] initWithTitle:@"View" action:nil keyEquivalent:@""];
  NSMenu* view_menu = [[NSMenu alloc] initWithTitle:@"View"];
  NSMenuItem* full_screen = [view_menu addItemWithTitle:@"Enter Full Screen"
                                                 action:@selector(toggleFullScreen:)
                                          keyEquivalent:@"f"];
  full_screen.keyEquivalentModifierMask =
      NSEventModifierFlagCommand | NSEventModifierFlagControl;
  view_item.submenu = view_menu;
  [main addItem:view_item];

  application.mainMenu = main;
}

}  // namespace darwin_art::window
