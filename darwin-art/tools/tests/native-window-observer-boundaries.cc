#include "compat/darwin_android_platform.h"
#include "compat/darwin_angle_egl.h"
#include "compat/darwin_surface_bridge.h"
#include "compat/graphics/hardware_buffer_owner.h"
#include <android/hardware_buffer.h>
#include <android/surface_control.h>
#include <cstdio>
#include <cstdlib>

// This fixture exercises producer observer/lease lifetime, not rendering.
// Unexpected transport/GPU/compositor use fails instead of fabricating success.
[[noreturn]] static void Unexpected(const char* name) {
  std::fprintf(stderr, "observer ownership fixture called %s\n", name);
  std::abort();
}
#define UNUSED_BOUNDARY(result, name, arguments) \
  extern "C" result name arguments { Unexpected(#name); }
UNUSED_BOUNDARY(int, AHardwareBuffer_allocate,
                (const AHardwareBuffer_Desc*, AHardwareBuffer**))
UNUSED_BOUNDARY(void, AHardwareBuffer_acquire, (AHardwareBuffer*))
UNUSED_BOUNDARY(void, AHardwareBuffer_release, (AHardwareBuffer*))
UNUSED_BOUNDARY(void, AHardwareBuffer_describe,
                (const AHardwareBuffer*, AHardwareBuffer_Desc*))
UNUSED_BOUNDARY(int, AHardwareBuffer_lock,
                (AHardwareBuffer*, uint64_t, int32_t, const ARect*, void**))
UNUSED_BOUNDARY(int, AHardwareBuffer_unlock, (AHardwareBuffer*, int32_t*))
UNUSED_BOUNDARY(void*, darwin_art_android_hardware_buffer_native_window_buffer,
                (AHardwareBuffer*))
UNUSED_BOUNDARY(AHardwareBuffer*, darwin_art_android_hardware_buffer_from_client_buffer,
                (void*))
UNUSED_BOUNDARY(void, darwin_art_android_hardware_buffer_mark_cpu_rgba,
                (AHardwareBuffer*))
UNUSED_BOUNDARY(void, ASurfaceControl_acquire, (ASurfaceControl*))
UNUSED_BOUNDARY(void, ASurfaceControl_release, (ASurfaceControl*))
UNUSED_BOUNDARY(void*, darwin_art_android_surface_control_create_root, (const char*))
UNUSED_BOUNDARY(void*, darwin_art_android_surface_control_create_imported,
                (uint32_t, uint32_t, const char*))
UNUSED_BOUNDARY(bool, darwin_art_android_surface_control_get_identity,
                (void*, uint32_t*, uint32_t*))
UNUSED_BOUNDARY(ASurfaceTransaction*, ASurfaceTransaction_create, ())
UNUSED_BOUNDARY(void, ASurfaceTransaction_delete, (ASurfaceTransaction*))
UNUSED_BOUNDARY(void, ASurfaceTransaction_apply, (ASurfaceTransaction*))
UNUSED_BOUNDARY(int, ASurfaceTransactionStats_getPreviousReleaseFenceFd,
                (ASurfaceTransactionStats*, ASurfaceControl*))
UNUSED_BOUNDARY(bool, ASurfaceTransactionStats_getPreviousBufferMetadata,
                (ASurfaceTransactionStats*, ASurfaceControl*, AHardwareBuffer**, uint64_t*))
UNUSED_BOUNDARY(bool, darwin_art_android_surface_transaction_set_buffer_with_cookie_checked,
                (void*, void*, AHardwareBuffer*, int, uint64_t))
UNUSED_BOUNDARY(bool, darwin_art_android_surface_transaction_set_buffer_callbacks_checked,
                (void*, void*, void*, void (*)(void*, ASurfaceTransactionStats*),
                 void (*)(void*, int)))
UNUSED_BOUNDARY(int, darwin_art_bionic_socket_broker_close, (int))
UNUSED_BOUNDARY(int, darwin_art_bionic_socket_broker_dup, (int))
UNUSED_BOUNDARY(int, sync_wait, (int, int))
UNUSED_BOUNDARY(DarwinArtSurface*, darwin_art_surface_active_gpu, ())
UNUSED_BOUNDARY(void, darwin_art_surface_gpu_publish_embedded, (DarwinArtSurface*))
#undef UNUSED_BOUNDARY
namespace darwin_art {
int DarwinAngleHostSurfaceWidth() { Unexpected("DarwinAngleHostSurfaceWidth"); }
int DarwinAngleHostSurfaceHeight() { Unexpected("DarwinAngleHostSurfaceHeight"); }
}  // namespace darwin_art
