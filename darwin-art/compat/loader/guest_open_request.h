#pragma once
#include "../../tools/android-dso-namespace/include/darwin_art_dso_namespace.h"

namespace darwin_art::loader {
// Android arm64 bionic libdl ABI (libc/include/dlfcn.h), not Darwin values.
inline constexpr int kAndroidRtldLazy = 0x1;
inline constexpr int kAndroidRtldNow = 0x2;
inline constexpr int kAndroidRtldNoload = 0x4;
inline constexpr int kAndroidRtldLocal = 0x0;
inline constexpr int kAndroidRtldGlobal = 0x100;
inline constexpr int kAndroidRtldNodelete = 0x1000;

// Parsed bionic dlopen request for the transitional guest loader callback.
// Binding mode is validated but has no further effect: bionic always resolves
// relocations eagerly, and so does the Darwin ELF graph loader.
struct GuestOpenRequest {
  // RTLD_GLOBAL: fixed at first load (soinfo_alloc rtld_flags); a later
  // reopen does not promote an already resident image.
  bool global = false;
  // RTLD_NODELETE: sticky; a reopen of a resident image promotes it.
  bool nodelete = false;
};

// Unsupported contracts must fail before cache/provider lookup or side effects.
// The namespace-backed linker must replace remaining limitations, not erase flags.
// Returns nullptr and fills `request` on success.
inline const char* ParseGuestOpenRequest(int flags, const DarwinArtAndroidDlExtInfo* info,
                                         GuestOpenRequest* request) {
  if (request == nullptr) return "invalid Android dlopen request output";
  *request = GuestOpenRequest{};
  if (info && info->flags != 0)
    return "legacy guest loader does not implement Android extended namespace/file loading";
  // bionic do_dlopen: "invalid flags to dlopen".
  constexpr int kKnown = kAndroidRtldLazy | kAndroidRtldNow | kAndroidRtldLocal |
                         kAndroidRtldGlobal | kAndroidRtldNodelete | kAndroidRtldNoload;
  if ((flags & ~kKnown) != 0) return "invalid Android dlopen flags";
  // RTLD_NOLOAD needs a side-effect-free SONAME/path identity query that the
  // legacy ClassLoader-backed callback does not have. Reject rather than load.
  if ((flags & kAndroidRtldNoload) != 0)
    return "legacy guest loader does not implement Android RTLD_NOLOAD";
  request->global = (flags & kAndroidRtldGlobal) != 0;
  request->nodelete = (flags & kAndroidRtldNodelete) != 0;
  return nullptr;
}

// bionic soinfo::can_unload: a linked image loaded RTLD_GLOBAL or marked
// RTLD_NODELETE is never unmapped by dlclose.
inline bool GuestImageUnloadable(bool global, bool nodelete) {
  return !global && !nodelete;
}
}
