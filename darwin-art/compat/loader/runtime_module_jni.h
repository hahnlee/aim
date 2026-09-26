#pragma once

namespace darwin_art::loader {

// The ART module's own native libraries are part of this runtime: its ART is
// a Darwin build, so their Android ELF copies (which bind to the image's
// libart) cannot run in this process. Returns this runtime image's handle
// when `soname`, requested by a class from the ART APEX, is one of them
// (ADR 0009); nullptr when the request is not such a library.
void* OpenRuntimeModuleJniLibrary(const char* soname, const char* caller_location,
                                  const char* android_filesystem_root);

}  // namespace darwin_art::loader
