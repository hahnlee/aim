#pragma once

#include <cstdint>

// The implementation owns the MTLSharedEvent and all descriptors created or
// consumed by the synchronization bridge. The C pointer is deliberately the
// actual MTLSharedEvent object because ANGLE/MoltenVK consume it in their
// native attribute ABI; its retained provider state is private to the registry.
namespace darwin_art::graphics {

void* CreateMetalSharedEvent(
    void* metal_device, std::uint64_t* first_signal_value) noexcept;
int MetalSharedEventFenceFd(void* event,
                            std::uint64_t signal_value) noexcept;
std::uint64_t MetalSharedEventNextValue(void* event) noexcept;

// Takes ownership of fence_fd, including on failure. A successful call
// admits a detached waiter whose retained event is released after sync_wait
// and exact descriptor cleanup complete.
int MetalSharedEventImportFence(void* event,
                                std::uint64_t signal_value,
                                int fence_fd) noexcept;
void ReleaseMetalSharedEvent(void* event) noexcept;

}  // namespace darwin_art::graphics

// Stable Android-facing ABI. Existing ANGLE, SurfaceFlinger, and Vulkan
// callers intentionally keep their local declarations and do not include
// this provider header.
extern "C" void* darwin_art_android_metal_shared_event_create(
    void* metal_device, std::uint64_t* signal_value);
extern "C" int darwin_art_android_metal_shared_event_fence_fd(
    void* shared_event, std::uint64_t signal_value);
extern "C" std::uint64_t darwin_art_android_metal_shared_event_next_value(
    void* shared_event);
extern "C" int darwin_art_android_metal_shared_event_import_fence(
    void* shared_event, std::uint64_t signal_value, int fence_fd);
extern "C" void darwin_art_android_metal_shared_event_release(
    void* shared_event);

#if defined(DARWIN_ART_METAL_SHARED_EVENT_TESTING)
// Isolated actual-TU tests use this admission seam to exercise the otherwise
// nondeterministic std::thread-constructor failure path. It is not exported
// by production builds.
extern "C" void darwin_art_android_metal_shared_event_reject_workers_for_test(
    bool reject);
#endif
