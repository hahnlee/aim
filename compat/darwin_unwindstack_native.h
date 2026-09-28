#pragma once

#include <cstddef>
#include <cstdint>

#include <unwindstack/DexFiles.h>

// Darwin has no /proc-based ART thread/TLS discovery equivalent. Keep the
// active generic-JNI quick frames in a fixed, allocation-free process record
// that a Mach task unwinder can read after stopping a thread.
inline constexpr uint64_t kAimQuickFrameRegistryVersion = 5;
inline constexpr size_t kAimQuickFrameRegistrySlots = 128;
inline constexpr size_t kAimQuickFrameRegistryDepth = 16;

struct AimQuickFrameSlot {
  uint64_t thread_id;
  uint64_t is_main_thread;
  uint64_t depth;
  uint64_t frames[kAimQuickFrameRegistryDepth];
  uint64_t frame_keys[kAimQuickFrameRegistryDepth];
  uint64_t frame_copies[kAimQuickFrameRegistryDepth][28];
  // 0 is the fixed SaveRefsAndArgs GenericJNI frame; 1 is a compiled-JNI
  // frame emitted by the ARM64 JNI compiler.  Keep this parallel array
  // fixed-size so a stopped remote task can read it without a heap object.
  uint64_t frame_kinds[kAimQuickFrameRegistryDepth];
  uint64_t frame_sizes[kAimQuickFrameRegistryDepth];
  uint64_t core_spill_masks[kAimQuickFrameRegistryDepth];
};

struct AimQuickFrameRegistry {
  uint64_t version;
  uint64_t slot_count;
  uint64_t art_main_thread_id;
  AimQuickFrameSlot slots[kAimQuickFrameRegistrySlots];
};

extern "C" AimQuickFrameRegistry aim_unwindstack_quick_frames;
extern "C" void aim_register_code_address(uintptr_t logical, uintptr_t host);
extern "C" void aim_register_native_method(uintptr_t entrypoint, const char* name);
// Publishes the actual JNI trampoline installed in an ArtMethod together with
// its guest ELF target.  NativeLoader registration bypasses ClassLinker in the
// host bridge, so this is the authoritative registration boundary there.
extern "C" void aim_register_native_entry_pair(uintptr_t entrypoint,
                                                        uintptr_t target,
                                                        const char* name);
extern "C" bool aim_lookup_native_method(uintptr_t entrypoint,
                                                   void (*callback)(const char*, void*),
                                                   void* context);
extern "C" void aim_unwindstack_set_art_main_thread();
// Refreshes a fixed, lock-free image-range table used by the ART signal path.
// The signal handler can query it without dyld/allocator calls.
extern "C" void aim_unwindstack_refresh_image_snapshot();
extern "C" bool aim_unwindstack_lookup_image(uintptr_t pc, uintptr_t* start,
                                                       uintptr_t* end);
extern "C" void aim_unwindstack_push_quick_frame(void* managed_sp);
extern "C" void aim_unwindstack_push_compiled_quick_frame(void* managed_sp,
                                                                    uint64_t frame_size,
                                                                    uint32_t core_spill_mask);
extern "C" void aim_unwindstack_pop_quick_frame();
extern "C" bool aim_unwindstack_pop_quick_frame_if(void* managed_sp,
                                                            uint64_t frame_kind);

namespace unwindstack {
class Elf;
template <typename Symfile>
class GlobalDebugInterface;
using JitDebug = GlobalDebugInterface<Elf>;
class Maps;
struct AndroidUnwinderData;
void DarwinRegisterAotCodeRange(const void* start, size_t size, uint64_t file_offset,
                                const char* oat_location);
void DarwinPublishAotCodeMaps(Maps* maps);
uint64_t DarwinFindGlobalVariable(Maps* maps, const char* variable);
bool DarwinNativeUnwind(Maps* maps, JitDebug* jit_debug, DexFiles* dex_files, size_t max_frames,
                        AndroidUnwinderData& data);
inline bool DarwinNativeUnwind(Maps* maps, JitDebug* jit_debug, size_t max_frames,
                               AndroidUnwinderData& data) {
  return DarwinNativeUnwind(maps, jit_debug, nullptr, max_frames, data);
}
bool DarwinNativeUnwindUcontext(Maps* maps, JitDebug* jit_debug, DexFiles* dex_files, size_t max_frames, void* ucontext,
                                AndroidUnwinderData& data);
bool DarwinNativeUnwindThread(Maps* maps, JitDebug* jit_debug, DexFiles* dex_files, size_t max_frames,
                              uint64_t thread_id,
                              AndroidUnwinderData& data);
bool DarwinNativeUnwindRemote(Maps* maps, JitDebug* jit_debug, DexFiles* dex_files, size_t max_frames, int process_id,
                              uint64_t thread_id,
                              AndroidUnwinderData& data);
}  // namespace unwindstack
