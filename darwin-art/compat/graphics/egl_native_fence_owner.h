#pragma once

#include <cstdint>

namespace darwin_art::graphics {

using EglNativeFenceBoolean = std::uint32_t;
using EglNativeFenceInt = std::int32_t;
using EglNativeFenceEnum = std::uint32_t;
using EglNativeFenceAttrib = std::intptr_t;
using EglNativeFenceDisplay = void*;
using EglNativeFenceSync = void*;

// This is a borrowed, already-resolved view of the ANGLE and Darwin provider
// operations needed by the Android native-fence adapter. The owner does not
// load libraries or retain callbacks; the facade keeps this backend valid for
// the duration of each call. Retired entries copy the cleanup callbacks, so
// the facade must keep the loaded ANGLE/provider DSOs live until owner cleanup
// has completed.
struct EglNativeFenceBackend {
  EglNativeFenceSync (*create_sync_khr)(EglNativeFenceDisplay,
                                        EglNativeFenceEnum,
                                        const EglNativeFenceInt*) = nullptr;
  EglNativeFenceBoolean (*destroy_sync)(EglNativeFenceDisplay,
                                        EglNativeFenceSync) = nullptr;
  EglNativeFenceBoolean (*destroy_sync_khr)(EglNativeFenceDisplay,
                                            EglNativeFenceSync) = nullptr;
  EglNativeFenceInt (*client_wait_sync)(EglNativeFenceDisplay,
                                        EglNativeFenceSync,
                                        EglNativeFenceInt,
                                        std::uint64_t) = nullptr;
  EglNativeFenceInt (*client_wait_sync_khr)(EglNativeFenceDisplay,
                                            EglNativeFenceSync,
                                            EglNativeFenceInt,
                                            std::uint64_t) = nullptr;
  EglNativeFenceBoolean (*wait_sync)(EglNativeFenceDisplay,
                                     EglNativeFenceSync,
                                     EglNativeFenceInt) = nullptr;
  EglNativeFenceBoolean (*wait_sync_khr)(EglNativeFenceDisplay,
                                         EglNativeFenceSync,
                                         EglNativeFenceInt) = nullptr;
  EglNativeFenceBoolean (*get_sync_attrib)(EglNativeFenceDisplay,
                                           EglNativeFenceSync,
                                           EglNativeFenceInt,
                                           EglNativeFenceAttrib*) = nullptr;
  EglNativeFenceBoolean (*get_sync_attrib_khr)(EglNativeFenceDisplay,
                                               EglNativeFenceSync,
                                               EglNativeFenceInt,
                                               EglNativeFenceInt*) = nullptr;
  EglNativeFenceInt (*dup_native_fence_fd)(EglNativeFenceDisplay,
                                           EglNativeFenceSync) = nullptr;
  EglNativeFenceInt (*dup_native_fence_fd_khr)(EglNativeFenceDisplay,
                                               EglNativeFenceSync) = nullptr;
  EglNativeFenceSync (*create_sync)(EglNativeFenceDisplay,
                                    EglNativeFenceEnum,
                                    const EglNativeFenceAttrib*) = nullptr;
  EglNativeFenceBoolean (*query_display_attrib)(EglNativeFenceDisplay,
                                                EglNativeFenceInt,
                                                EglNativeFenceAttrib*) = nullptr;
  EglNativeFenceBoolean (*query_device_attrib)(void*, EglNativeFenceInt,
                                               EglNativeFenceAttrib*) = nullptr;

  void* (*metal_shared_event_create)(void*, std::uint64_t*) = nullptr;
  int (*metal_shared_event_fence_fd)(void*, std::uint64_t) = nullptr;
  void (*metal_shared_event_release)(void*) = nullptr;
  int (*sync_wait)(int, int) = nullptr;
  int (*close_fence_fd)(int) = nullptr;
  void (*synchronize_iosurface_to_ahb)() = nullptr;
  void (*synchronize_ahb_to_iosurface)() = nullptr;
  bool debug = false;
};

struct EglNativeFenceExport {
  EglNativeFenceSync token = nullptr;
  void* shared_event = nullptr;
  std::uint64_t signal_value = 0;
};

// The owner is process-local because EGLSync handles are opaque pointers whose
// lifetime is bounded by the loaded ANGLE process.  Its registry and per-entry
// admission state remain private; callers receive only ABI handles/metadata.
EglNativeFenceSync CreateNativeFenceSync(
    EglNativeFenceDisplay display, EglNativeFenceEnum type,
    const EglNativeFenceInt* attributes, const EglNativeFenceBackend& backend);
EglNativeFenceBoolean DestroyNativeFenceSync(
    EglNativeFenceDisplay display, EglNativeFenceSync sync,
    const EglNativeFenceBackend& backend);
EglNativeFenceInt ClientWaitNativeFenceSync(
    EglNativeFenceDisplay display, EglNativeFenceSync sync,
    EglNativeFenceInt flags, std::uint64_t timeout,
    const EglNativeFenceBackend& backend);
EglNativeFenceBoolean WaitNativeFenceSync(
    EglNativeFenceDisplay display, EglNativeFenceSync sync,
    EglNativeFenceInt flags, const EglNativeFenceBackend& backend);
EglNativeFenceBoolean GetNativeFenceSyncAttrib(
    EglNativeFenceDisplay display, EglNativeFenceSync sync,
    EglNativeFenceInt attribute, EglNativeFenceInt* value,
    const EglNativeFenceBackend& backend);
EglNativeFenceInt DupNativeFenceFd(
    EglNativeFenceDisplay display, EglNativeFenceSync sync,
    const EglNativeFenceBackend& backend);
EglNativeFenceExport ExportNativeFence(
    EglNativeFenceDisplay display, const EglNativeFenceBackend& backend);
void ReleaseNativeFence(EglNativeFenceDisplay display,
                        EglNativeFenceSync token,
                        const EglNativeFenceBackend& backend);

#if defined(DARWIN_ART_EGL_NATIVE_FENCE_OWNER_TESTING)
// Isolated owner-TU tests can deterministically exercise allocation failure
// before permanent-record allocation or Entry construction. This is not part
// of the production ABI.
void SetNativeFenceOwnerAllocationFailuresForTest(
    int record_failures, int entry_failures, int index_failures = 0) noexcept;
#endif

}  // namespace darwin_art::graphics
