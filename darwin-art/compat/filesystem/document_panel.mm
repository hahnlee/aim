#import <AppKit/AppKit.h>

#include "document_panel.h"

#include <cstdlib>
#include <cstring>

namespace {

bool IsMainThread() {
  return [NSThread isMainThread];
}

}  // namespace

char* darwin_art_host_open_document(const char* mime_type) {
  if (!IsMainThread()) return nullptr;
  @autoreleasepool {
    NSOpenPanel* panel = [NSOpenPanel openPanel];
    panel.canChooseFiles = YES;
    panel.canChooseDirectories = NO;
    panel.allowsMultipleSelection = NO;
    panel.resolvesAliases = YES;
    panel.title = @"Open Android document";
    panel.prompt = @"Open";
    if (mime_type != nullptr && strncmp(mime_type, "image/", 6) == 0) {
      panel.allowedFileTypes = @[@"jpg", @"jpeg", @"png", @"gif", @"webp"];
    }
    if ([panel runModal] != NSModalResponseOK || panel.URL == nil) {
      return nullptr;
    }
    const char* path = panel.URL.fileSystemRepresentation;
    return path == nullptr ? nullptr : strdup(path);
  }
}

char* darwin_art_host_save_document(const char* mime_type,
                                    const char* suggested_name) {
  if (!IsMainThread()) return nullptr;
  @autoreleasepool {
    NSSavePanel* panel = [NSSavePanel savePanel];
    panel.canCreateDirectories = YES;
    panel.title = @"Save Android document";
    panel.prompt = @"Save";
    if (mime_type != nullptr && strncmp(mime_type, "image/", 6) == 0) {
      panel.allowedFileTypes = @[@"jpg", @"jpeg", @"png", @"gif", @"webp"];
    }
    if (suggested_name != nullptr && suggested_name[0] != '\0') {
      NSString* name = [NSString stringWithUTF8String:suggested_name];
      if (name != nil) panel.nameFieldStringValue = name;
    }
    if ([panel runModal] != NSModalResponseOK || panel.URL == nil) {
      return nullptr;
    }
    const char* path = panel.URL.fileSystemRepresentation;
    return path == nullptr ? nullptr : strdup(path);
  }
}

void darwin_art_host_document_path_free(char* path) {
  free(path);
}
