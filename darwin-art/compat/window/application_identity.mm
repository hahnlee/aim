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

}  // namespace darwin_art::window
