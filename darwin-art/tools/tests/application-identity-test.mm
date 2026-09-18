#import <AppKit/AppKit.h>

#include "compat/window/application_identity.h"

#include <cassert>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <unistd.h>

namespace {

using darwin_art::window::ApplySurfaceApplicationIdentity;
using darwin_art::window::DecodeSurfaceWindowTitle;

void CheckGenericTitle(const char* title) {
  NSString* decoded = DecodeSurfaceWindowTitle(title);
  assert([decoded isEqualToString:@"Darwin ART Surface"]);
}

}  // namespace

int main() {
  @autoreleasepool {
    assert([DecodeSurfaceWindowTitle(
                "\xEA\xB3\x84\xEC\x82\xB0\xEA\xB8\xB0")
                isEqualToString:@"계산기"]);
    CheckGenericTitle(nullptr);
    CheckGenericTitle("");
    const char invalid_utf8[] = {'x', static_cast<char>(0xff), '\0'};
    CheckGenericTitle(invalid_utf8);

    NSApplication* application = [NSApplication sharedApplication];
    NSWindow* window = [[NSWindow alloc]
        initWithContentRect:NSMakeRect(0, 0, 320, 240)
                  styleMask:NSWindowStyleMaskBorderless
                    backing:NSBackingStoreBuffered
                      defer:YES];
    assert(application != nil && window != nil);

    NSImage* sentinel = [[NSImage alloc] initWithSize:NSMakeSize(7, 9)];
    application.applicationIconImage = sentinel;
    window.miniwindowImage = sentinel;
    NSImage* application_before = application.applicationIconImage;
    NSImage* window_before = window.miniwindowImage;
    unsetenv("DARWIN_ART_APK_APP_ICON");
    ApplySurfaceApplicationIdentity(application, window);
    assert(application.applicationIconImage == application_before);
    assert(window.miniwindowImage == window_before);

    char path[] = "/tmp/darwin-art-application-icon-XXXXXX";
    const int fd = mkstemp(path);
    assert(fd >= 0);
    close(fd);
    NSBitmapImageRep* source = [[NSBitmapImageRep alloc]
        initWithBitmapDataPlanes:nullptr
                       pixelsWide:32
                       pixelsHigh:16
                    bitsPerSample:8
                  samplesPerPixel:4
                         hasAlpha:YES
                         isPlanar:NO
                   colorSpaceName:NSDeviceRGBColorSpace
                      bitmapFormat:0
                       bytesPerRow:0
                      bitsPerPixel:0];
    assert(source != nil);
    NSData* png = [source representationUsingType:NSBitmapImageFileTypePNG
                                         properties:@{}];
    assert(png != nil && [png writeToFile:[NSString stringWithUTF8String:path]
                                 atomically:YES]);
    setenv("DARWIN_ART_APK_APP_ICON", path, 1);
    ApplySurfaceApplicationIdentity(application, window);
    NSImage* icon = application.applicationIconImage;
    NSImage* window_icon = window.miniwindowImage;
    assert(icon != nil && window_icon != nil);
    assert(NSEqualSizes(icon.size, NSMakeSize(128.0, 128.0)));
    assert(NSEqualSizes(window_icon.size, NSMakeSize(128.0, 128.0)));
    NSArray<NSImageRep*>* representations = window_icon.representations;
    assert(representations.count > 0);
    NSImageRep* representation = representations.firstObject;
    assert(representation.pixelsWide == 32 && representation.pixelsHigh == 16);

    unlink(path);
    unsetenv("DARWIN_ART_APK_APP_ICON");
    window = nil;
    application.applicationIconImage = nil;
    std::puts("application identity: UTF-8 title/fallback, icon path/missing, native pixels PASS");
    return 0;
  }
}
