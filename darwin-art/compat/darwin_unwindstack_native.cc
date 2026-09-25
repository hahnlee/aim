#include "darwin_unwindstack_native.h"

#include <dlfcn.h>
#include <libunwind.h>
#include <mach/arm/thread_status.h>
#include <mach/mach.h>
#include <mach-o/dyld.h>
#include <mach-o/loader.h>
#include <mach-o/nlist.h>
#include <ptrauth.h>
#include <pthread.h>
#include <sys/mman.h>
#include <unwind.h>

#include <array>
#include <atomic>
#include <cstdlib>
#include <cxxabi.h>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <mutex>
#include <string>
#include <string_view>
#include <unordered_set>
#include <utility>
#include <vector>

#include <unwindstack/AndroidUnwinder.h>
#include <unwindstack/JitDebug.h>
#include <unwindstack/MachineArm64.h>
#include <unwindstack/Maps.h>
#include <unwindstack/Memory.h>
#include <unwindstack/RegsArm64.h>
#include <unwindstack/Unwinder.h>

#include "darwin_remote_task_cache.h"

namespace android {
// The standalone unwindstack provider smoke binary does not link the ART
// thread owner. Keep a weak no-op there; the runtime's strong implementation
// in darwin_jni_shorty.cc supplies the real ManagedStack query.
__attribute__((weak)) bool CurrentGenericJniFrame(uint64_t* managed_sp) {
  (void)managed_sp;
  return false;
}
__attribute__((weak)) bool CurrentInterpreterFrame(uint64_t* shadow_frame) {
  (void)shadow_frame;
  return false;
}
}  // namespace android

extern "C" __attribute__((visibility("default"))) DarwinArtQuickFrameRegistry
    darwin_art_unwindstack_quick_frames = {
        kDarwinArtQuickFrameRegistryVersion, kDarwinArtQuickFrameRegistrySlots, 0, {}};

DarwinArtQuickFrameRegistry* SharedQuickFrameRegistry() {
  static DarwinArtQuickFrameRegistry* registry = [] {
    void* address = dlsym(RTLD_DEFAULT, "darwin_art_unwindstack_quick_frames");
    auto* selected = address == nullptr ? &darwin_art_unwindstack_quick_frames
                                        : static_cast<DarwinArtQuickFrameRegistry*>(address);
    return selected;
  }();
  return registry;
}

namespace {

constexpr size_t kDarwinImageSnapshotCapacity = 512;
struct DarwinImageSnapshotEntry {
  uintptr_t start;
  uintptr_t end;
};
std::array<DarwinImageSnapshotEntry, kDarwinImageSnapshotCapacity> g_image_snapshot{};
std::atomic<size_t> g_image_snapshot_count{0};

void RefreshImageSnapshot() {
  size_t count = 0;
  for (uint32_t image = 0; image < _dyld_image_count() && count < kDarwinImageSnapshotCapacity;
       ++image) {
    const auto* header = reinterpret_cast<const mach_header_64*>(_dyld_get_image_header(image));
    if (header == nullptr || header->magic != MH_MAGIC_64) continue;
    const intptr_t slide = _dyld_get_image_vmaddr_slide(image);
    const auto* bytes = reinterpret_cast<const uint8_t*>(header + 1);
    for (uint32_t i = 0; i < header->ncmds && count < kDarwinImageSnapshotCapacity; ++i) {
      const auto* command = reinterpret_cast<const load_command*>(bytes);
      if (command->cmd == LC_SEGMENT_64) {
        const auto* segment = reinterpret_cast<const segment_command_64*>(command);
        if (segment->vmsize != 0) {
          g_image_snapshot[count++] = {
              static_cast<uintptr_t>(segment->vmaddr + slide),
              static_cast<uintptr_t>(segment->vmaddr + slide + segment->vmsize)};
        }
      }
      bytes += command->cmdsize;
    }
  }
  g_image_snapshot_count.store(count, std::memory_order_release);
}

}  // namespace

extern "C" void darwin_art_unwindstack_refresh_image_snapshot() { RefreshImageSnapshot(); }

extern "C" bool darwin_art_unwindstack_lookup_image(uintptr_t pc, uintptr_t* start,
                                                       uintptr_t* end) {
  if (start == nullptr || end == nullptr) return false;
  const size_t count = g_image_snapshot_count.load(std::memory_order_acquire);
  for (size_t i = 0; i < count; ++i) {
    const auto entry = g_image_snapshot[i];
    if (pc >= entry.start && pc < entry.end) {
      *start = entry.start;
      *end = entry.end;
      return true;
    }
  }
  return false;
}

namespace {

// A Mach thread can carry a nested suspend count when another profiler or
// signal path sampled it concurrently.  Resume every suspend level acquired
// by the provider so a successful unwind can never strand the target thread.
void ResumeThreadFully(thread_t thread) {
  for (unsigned attempt = 0; attempt < 4; ++attempt) {
    if (thread_resume(thread) != KERN_SUCCESS) break;
  }
}

// ART image/code references retain Android's low 32-bit logical address on
// Darwin.  Only lift a managed PC when the corresponding host-window address
// is actually mapped; this avoids inventing aliases for unrelated low PCs.
constexpr uint64_t kDarwinArtCompressedReferenceBase = 0x0000010000000000ULL;

uint64_t NormalizeManagedPc(unwindstack::Maps* maps, uint64_t pc);

struct DarwinAotCodeRange {
  uint64_t start;
  uint64_t end;
  uint64_t file_offset;
  std::string oat_location;
};

// GC stress asks for a backtrace at every allocation.  Managed allocation
// loops repeatedly enter the same quick frame, so retain the last completed
// managed walk per host thread.  The managed SP is part of the key to avoid
// reusing a loop's trace across recursive or concurrently changing frames.
struct LocalManagedBacktraceCache {
  uint64_t caller_pc = 0;
  uint64_t frame_pointer = 0;
  uint64_t managed_sp = 0;
  uint64_t shadow_frame = 0;
  std::vector<unwindstack::FrameData> frames;
};

thread_local LocalManagedBacktraceCache g_local_managed_backtrace_cache;
std::mutex g_aot_ranges_mutex;
std::vector<DarwinAotCodeRange> g_aot_ranges;
struct DarwinCodeAddressPair {
  uint64_t logical;
  uint64_t host;
};
std::vector<DarwinCodeAddressPair> g_code_address_pairs;
struct DarwinCompiledMethodRange {
  uint64_t start;
  uint64_t end;
  std::string name;
};
std::vector<DarwinCompiledMethodRange> g_compiled_method_ranges;
std::vector<std::pair<uint64_t, std::string>> g_native_method_names;
struct DarwinNativeEntryPair {
  uint64_t entrypoint;
  uint64_t target;
};
std::vector<DarwinNativeEntryPair> g_native_entry_pairs;

uint64_t NormalizeManagedPc(unwindstack::Maps* maps, uint64_t pc) {
  if (maps == nullptr || pc >= (1ULL << 32)) return pc;
  {
    std::lock_guard<std::mutex> lock(g_aot_ranges_mutex);
    for (const auto& pair : g_code_address_pairs) {
      if (pc >= pair.logical && pc - pair.logical < (1ULL << 20)) {
        const uint64_t candidate = pair.host + (pc - pair.logical);
        const auto mapping = maps->Find(candidate);
        if (mapping != nullptr && (mapping->flags() & PROT_EXEC) != 0) return candidate;
      }
    }
    // ART's app OAT code is allocated in a 1MiB-aligned logical segment. When
    // the 32-bit pointer field has discarded the segment base, recover the
    // intra-segment offset against each registered host OAT range and accept
    // only a candidate that is actually executable and inside that range.
    const uint64_t logical_segment = pc & ~((1ULL << 20) - 1);
    const uint64_t segment_offset = pc - logical_segment;
    for (const auto& range : g_aot_ranges) {
      // A low app PC can share the same 1MiB segment offset as boot-image
      // code. Prefer the application OAT range; boot-image candidates would
      // make the managed unwinder resolve a valid but unrelated method.
      if (range.oat_location.find("boot-image") != std::string::npos) continue;
      const uint64_t candidate = range.start + segment_offset;
      if (candidate < range.end) {
        const auto mapping = maps->Find(candidate);
        if (mapping != nullptr && (mapping->flags() & PROT_EXEC) != 0) {
          if (mapping->name().empty()) {
            mapping->set_name(range.oat_location.c_str());
            mapping->set_offset(range.file_offset);
          }
          return candidate;
        }
      }
    }
    // The runtime and the standalone CFI probe can load this provider archive
    // into separate images, so their in-process AOT registries are not always
    // shared. Recover the same segment offset directly from the process maps
    // when an executable OAT/ODEX mapping is visible to this unwinder.
    uint64_t mapped_candidate = 0;
    maps->ForEachMapInfo([&](unwindstack::MapInfo* map) {
      if (mapped_candidate != 0 || map == nullptr ||
          (map->flags() & PROT_EXEC) == 0 || map->end() <= map->start() ||
          static_cast<std::string_view>(map->name()).find(".oat") == std::string::npos &&
              static_cast<std::string_view>(map->name()).find(".odex") == std::string::npos) {
        return true;
      }
      const uint64_t candidate = map->start() + segment_offset;
      if (candidate < map->end()) {
        mapped_candidate = candidate;
        if (std::getenv("DARWIN_ART_DEBUG_CFI") != nullptr) {
          std::fprintf(stderr,
                       "darwin-cfi: map-candidate pc=%llx start=%llx end=%llx candidate=%llx name=%s\\n",
                       static_cast<unsigned long long>(pc),
                       static_cast<unsigned long long>(map->start()),
                       static_cast<unsigned long long>(map->end()),
                       static_cast<unsigned long long>(candidate), map->name().c_str());
        }
      }
      return true;
    });
    if (mapped_candidate != 0) return mapped_candidate;
  }
  const uint64_t candidate = kDarwinArtCompressedReferenceBase + pc;
  const auto mapping = maps->Find(candidate);
  return mapping != nullptr && (mapping->flags() & PROT_EXEC) != 0 ? candidate : pc;
}

DarwinArtQuickFrameSlot* FindLocalQuickFrameSlot(uint64_t thread_id, bool claim) {
  const size_t first = thread_id % kDarwinArtQuickFrameRegistrySlots;
  DarwinArtQuickFrameRegistry* registry = SharedQuickFrameRegistry();
  for (size_t probe = 0; probe < kDarwinArtQuickFrameRegistrySlots; ++probe) {
    auto* slot = &registry->slots[(first + probe) % kDarwinArtQuickFrameRegistrySlots];
    uint64_t owner = __atomic_load_n(&slot->thread_id, __ATOMIC_ACQUIRE);
    if (owner == thread_id) return slot;
    if (claim && owner == 0 && __atomic_compare_exchange_n(&slot->thread_id, &owner, thread_id,
                                                           false, __ATOMIC_ACQ_REL,
                                                           __ATOMIC_ACQUIRE)) {
      return slot;
    }
  }
  return nullptr;
}

bool ReadLocalQuickFrame(uint64_t thread_id, uint64_t* managed_sp, uint64_t* frame_kind,
                         uint64_t* frame_size, uint64_t* core_spill_mask) {
  if (thread_id == 0 || managed_sp == nullptr || frame_kind == nullptr || frame_size == nullptr ||
      core_spill_mask == nullptr) {
    return false;
  }
  DarwinArtQuickFrameSlot* slot = FindLocalQuickFrameSlot(thread_id, false);
  if (slot == nullptr) return false;
  const uint64_t depth = __atomic_load_n(&slot->depth, __ATOMIC_ACQUIRE);
  if (depth == 0 || depth > kDarwinArtQuickFrameRegistryDepth) return false;
  const size_t index = static_cast<size_t>(depth - 1);
  *managed_sp = __atomic_load_n(&slot->frames[index], __ATOMIC_ACQUIRE);
  *frame_kind = __atomic_load_n(&slot->frame_kinds[index], __ATOMIC_ACQUIRE);
  *frame_size = __atomic_load_n(&slot->frame_sizes[index], __ATOMIC_ACQUIRE);
  *core_spill_mask = __atomic_load_n(&slot->core_spill_masks[index], __ATOMIC_ACQUIRE);
  return *managed_sp != 0;
}

}  // namespace

extern "C" void darwin_art_register_code_address(uintptr_t logical, uintptr_t host) {
  if (logical == 0 || host == 0 || logical >= (1ULL << 32)) return;
  if (std::getenv("DARWIN_ART_DEBUG_CFI") != nullptr) {
    std::fprintf(stderr, "darwin-cfi: entry-pair logical=%llx host=%llx\\n",
                 static_cast<unsigned long long>(logical),
                 static_cast<unsigned long long>(host));
  }
  std::lock_guard<std::mutex> lock(g_aot_ranges_mutex);
  for (const auto& pair : g_code_address_pairs) {
    if (pair.logical == logical && pair.host == host) return;
  }
  g_code_address_pairs.push_back({logical, host});
}

extern "C" void darwin_art_register_compiled_method(uintptr_t start, size_t size,
                                                      const char* name) {
  if (start == 0 || size == 0 || name == nullptr || *name == '\0') return;
  std::lock_guard<std::mutex> lock(g_aot_ranges_mutex);
  const uint64_t end = start + size;
  for (auto& range : g_compiled_method_ranges) {
    if (range.start == start && range.end == end) {
      range.name = name;
      return;
    }
  }
  g_compiled_method_ranges.push_back({start, end, name});
}

extern "C" void darwin_art_register_native_method(uintptr_t entrypoint, const char* name) {
  if (entrypoint == 0 || name == nullptr || *name == '\0') return;
  std::lock_guard<std::mutex> lock(g_aot_ranges_mutex);
  for (auto& item : g_native_method_names) {
    if (item.first == entrypoint) {
      item.second = name;
      return;
    }
  }
  g_native_method_names.emplace_back(entrypoint, name);
}

extern "C" bool darwin_art_lookup_native_method(uintptr_t entrypoint,
                                                   void (*callback)(const char*, void*),
                                                   void* context) {
  if (entrypoint == 0 || callback == nullptr) return false;
  std::lock_guard<std::mutex> lock(g_aot_ranges_mutex);
  for (const auto& item : g_native_method_names) {
    if (item.first == entrypoint) {
      callback(item.second.c_str(), context);
      return true;
    }
  }
  return false;
}

extern "C" void darwin_art_register_native_entry_pair(uintptr_t entrypoint,
                                                        uintptr_t target,
                                                        const char* name) {
  if (entrypoint == 0 || target == 0 || name == nullptr || *name == '\0') return;
  std::lock_guard<std::mutex> lock(g_aot_ranges_mutex);
  bool found = false;
  for (auto& pair : g_native_entry_pairs) {
    if (pair.entrypoint == entrypoint) {
      pair.target = target;
      found = true;
      break;
    }
  }
  if (!found) g_native_entry_pairs.push_back({entrypoint, target});
  for (auto& item : g_native_method_names) {
    if (item.first == entrypoint) {
      item.second = name;
      return;
    }
  }
  g_native_method_names.emplace_back(entrypoint, name);
}

bool FindCompiledMethodName(uint64_t pc, std::string* name) {
  std::lock_guard<std::mutex> lock(g_aot_ranges_mutex);
  for (const auto& range : g_compiled_method_ranges) {
    if (pc >= range.start && pc < range.end) {
      *name = range.name;
      return true;
    }
  }
  return false;
}

namespace unwindstack {

void DarwinRegisterAotCodeRange(const void* start, size_t size, uint64_t file_offset,
                                const char* oat_location) {
  if (start == nullptr || size == 0 || oat_location == nullptr || *oat_location == '\0') return;
  std::lock_guard<std::mutex> lock(g_aot_ranges_mutex);
  const uint64_t begin = reinterpret_cast<uint64_t>(start);
  const uint64_t end = begin + size;
  for (const auto& range : g_aot_ranges) {
    if (range.start == begin && range.end == end && range.file_offset == file_offset &&
        range.oat_location == oat_location) return;
  }
  g_aot_ranges.push_back({begin, end, file_offset, oat_location});
  if (std::getenv("DARWIN_ART_DEBUG_CFI") != nullptr) {
    std::fprintf(stderr, "darwin-cfi: aot-range start=%llx end=%llx file=%llx path=%s\\n",
                 static_cast<unsigned long long>(begin), static_cast<unsigned long long>(end),
                 static_cast<unsigned long long>(file_offset), oat_location);
  }
}

void DarwinPublishAotCodeMaps(Maps* maps) {
  if (maps == nullptr) return;
  std::lock_guard<std::mutex> lock(g_aot_ranges_mutex);
  bool changed = false;
  for (const auto& range : g_aot_ranges) {
    if (maps->Find(range.start) == nullptr) {
      maps->Add(range.start, range.end, range.file_offset, PROT_READ | PROT_EXEC,
                range.oat_location);
      changed = true;
    }
  }
  // CheckGcStressMode can request a backtrace for every allocation.  The
  // AOT registry is normally stable between those calls, so repeatedly
  // sorting the per-thread map is pure overhead.  Keep the map sorted when
  // its contents change and make the steady-state publication O(ranges)
  // without changing lookup semantics.
  if (changed) maps->Sort();
}

}  // namespace unwindstack

extern "C" __attribute__((visibility("default"))) void
darwin_art_unwindstack_set_art_main_thread() {
  RefreshImageSnapshot();
  uint64_t thread_id = 0;
  if (pthread_threadid_np(nullptr, &thread_id) == 0 && thread_id != 0) {
    __atomic_store_n(&SharedQuickFrameRegistry()->art_main_thread_id, thread_id,
                     __ATOMIC_RELEASE);
  }
}

extern "C" __attribute__((visibility("default"))) void
darwin_art_unwindstack_push_quick_frame(void* managed_sp) {
  uint64_t thread_id = 0;
  if (managed_sp == nullptr || pthread_threadid_np(nullptr, &thread_id) != 0 || thread_id == 0) {
    return;
  }
  // A managed quick frame is 16-byte aligned on this thread's stack, and the
  // copied frame words below must lie inside it. Anything else is not a
  // managed SP (for example x28 under a compiled JNI stub).
  const uintptr_t sp = reinterpret_cast<uintptr_t>(managed_sp);
  const uintptr_t stack_top = reinterpret_cast<uintptr_t>(pthread_get_stackaddr_np(pthread_self()));
  const uintptr_t stack_size = pthread_get_stacksize_np(pthread_self());
  if ((sp & 15u) != 0 || stack_size > stack_top || sp < stack_top - stack_size ||
      sp > stack_top - 28u * sizeof(uint64_t)) {
    return;
  }
  DarwinArtQuickFrameSlot* slot = FindLocalQuickFrameSlot(thread_id, true);
  if (slot == nullptr) return;
  const uint64_t art_main_thread_id = __atomic_load_n(
      &SharedQuickFrameRegistry()->art_main_thread_id, __ATOMIC_ACQUIRE);
  __atomic_store_n(&slot->is_main_thread, art_main_thread_id == thread_id ? uint64_t{1} : uint64_t{0},
                   __ATOMIC_RELAXED);
  const uint64_t depth = __atomic_load_n(&slot->depth, __ATOMIC_RELAXED);
  if (depth >= kDarwinArtQuickFrameRegistryDepth) return;
  // Generic-JNI publication can be reached through both the trampoline and
  // the common JNI method-start hook. Treat a repeated publication of the
  // same managed frame as idempotent so the matching method-end pop cannot
  // leave a stale registry entry behind.
  if (depth != 0 &&
      __atomic_load_n(&slot->frame_keys[depth - 1], __ATOMIC_RELAXED) ==
          reinterpret_cast<uint64_t>(managed_sp) &&
      __atomic_load_n(&slot->frame_kinds[depth - 1], __ATOMIC_RELAXED) == 0) {
    return;
  }
  for (size_t i = 0; i < 28; ++i) {
    __atomic_store_n(&slot->frame_copies[depth][i],
                     reinterpret_cast<const uint64_t*>(managed_sp)[i], __ATOMIC_RELAXED);
  }
  __atomic_store_n(&slot->frame_keys[depth], reinterpret_cast<uint64_t>(managed_sp), __ATOMIC_RELAXED);
  __atomic_store_n(&slot->frames[depth],
                   reinterpret_cast<uint64_t>(&slot->frame_copies[depth][0]), __ATOMIC_RELAXED);
  __atomic_store_n(&slot->frame_kinds[depth], uint64_t{0}, __ATOMIC_RELAXED);
  __atomic_store_n(&slot->frame_sizes[depth], uint64_t{224}, __ATOMIC_RELAXED);
  __atomic_store_n(&slot->core_spill_masks[depth], uint64_t{0}, __ATOMIC_RELAXED);
  __atomic_store_n(&slot->depth, depth + 1, __ATOMIC_RELEASE);
}

extern "C" __attribute__((visibility("default"))) void
darwin_art_unwindstack_push_compiled_quick_frame(void* managed_sp, uint64_t frame_size,
                                                 uint32_t core_spill_mask) {
  uint64_t thread_id = 0;
  if (managed_sp == nullptr || pthread_threadid_np(nullptr, &thread_id) != 0 || thread_id == 0) {
    return;
  }
  DarwinArtQuickFrameSlot* slot = FindLocalQuickFrameSlot(thread_id, true);
  if (slot == nullptr) return;
  const uint64_t art_main_thread_id = __atomic_load_n(
      &SharedQuickFrameRegistry()->art_main_thread_id, __ATOMIC_ACQUIRE);
  __atomic_store_n(&slot->is_main_thread, art_main_thread_id == thread_id ? uint64_t{1} : uint64_t{0},
                   __ATOMIC_RELAXED);
  const uint64_t depth = __atomic_load_n(&slot->depth, __ATOMIC_RELAXED);
  if (depth >= kDarwinArtQuickFrameRegistryDepth) return;
  __atomic_store_n(&slot->frames[depth], reinterpret_cast<uint64_t>(managed_sp), __ATOMIC_RELAXED);
  __atomic_store_n(&slot->frame_kinds[depth], uint64_t{1}, __ATOMIC_RELAXED);
  __atomic_store_n(&slot->frame_sizes[depth], frame_size, __ATOMIC_RELAXED);
  __atomic_store_n(&slot->core_spill_masks[depth], core_spill_mask, __ATOMIC_RELAXED);
  __atomic_store_n(&slot->depth, depth + 1, __ATOMIC_RELEASE);
}

extern "C" __attribute__((visibility("default"))) void darwin_art_unwindstack_pop_quick_frame() {
  uint64_t thread_id = 0;
  if (pthread_threadid_np(nullptr, &thread_id) != 0 || thread_id == 0) return;
  DarwinArtQuickFrameSlot* slot = FindLocalQuickFrameSlot(thread_id, false);
  if (slot == nullptr) return;
  const uint64_t depth = __atomic_load_n(&slot->depth, __ATOMIC_ACQUIRE);
  if (depth == 0) return;
  __atomic_store_n(&slot->depth, depth - 1, __ATOMIC_RELEASE);
  if (depth == 1) {
    __atomic_store_n(&slot->thread_id, uint64_t{0}, __ATOMIC_RELEASE);
  }
}

extern "C" __attribute__((visibility("default"))) bool
darwin_art_unwindstack_pop_quick_frame_if(void* managed_sp, uint64_t frame_kind) {
  uint64_t thread_id = 0;
  if (managed_sp == nullptr || pthread_threadid_np(nullptr, &thread_id) != 0 || thread_id == 0) {
    return false;
  }
  DarwinArtQuickFrameSlot* slot = FindLocalQuickFrameSlot(thread_id, false);
  if (slot == nullptr) return false;
  const uint64_t depth = __atomic_load_n(&slot->depth, __ATOMIC_ACQUIRE);
  if (depth == 0) return false;
  const uint64_t index = depth - 1;
  if (__atomic_load_n(&slot->frame_keys[index], __ATOMIC_RELAXED) !=
          reinterpret_cast<uint64_t>(managed_sp) ||
      __atomic_load_n(&slot->frame_kinds[index], __ATOMIC_RELAXED) != frame_kind) {
    return false;
  }
  __atomic_store_n(&slot->depth, index, __ATOMIC_RELEASE);
  if (index == 0) {
    __atomic_store_n(&slot->thread_id, uint64_t{0}, __ATOMIC_RELEASE);
  }
  return true;
}

namespace unwindstack {
namespace {

struct NativeWalk {
  Maps* maps;
  JitDebug* jit_debug;
  DexFiles* dex_files;
  AndroidUnwinderData* data;
  size_t limit;
  mach_port_t task;
  std::shared_ptr<Memory> memory;
  uint64_t last_cfa = 0;
  uint64_t last_x28 = 0;
  uint64_t registered_managed_sp = 0;
  uint64_t registered_frame_kind = 0;
  uint64_t registered_frame_size = 0;
  uint64_t registered_core_spill_mask = 0;
  bool has_registered_quick_frame = false;
  std::array<uint64_t, ARM64_REG_R30 + 1> last_registers{};
};

uint64_t StripReturnAddress(uint64_t address);

bool LookupNativeSymbol(Maps* maps, uint64_t pc, Dl_info* symbol) {
  if (dladdr(reinterpret_cast<const void*>(pc), symbol) != 0 && symbol->dli_sname != nullptr) {
    return true;
  }
  if (maps == nullptr) return false;
  auto map = maps->Find(pc);
  if (map == nullptr || map->name().empty()) return false;
  uint64_t remote_base = 0;
  maps->ForEachMapInfo([&](MapInfo* candidate) {
    if (candidate->name() == map->name() && candidate->offset() == 0) {
      remote_base = candidate->start();
      return false;
    }
    return true;
  });
  if (remote_base == 0 || pc < remote_base) return false;
  for (uint32_t image = 0; image < _dyld_image_count(); ++image) {
    const char* path = _dyld_get_image_name(image);
    const auto* header = _dyld_get_image_header(image);
    if (path == nullptr || header == nullptr || map->name() != path) continue;
    const uint64_t translated = reinterpret_cast<uint64_t>(header) + (pc - remote_base);
    return dladdr(reinterpret_cast<const void*>(translated), symbol) != 0 &&
           symbol->dli_sname != nullptr;
  }
  return false;
}

bool LookupMachOSymbol(Maps* maps, uint64_t pc, std::string* name, uint64_t* function_offset) {
  if (maps == nullptr || name == nullptr || function_offset == nullptr) return false;
  auto map = maps->Find(pc);
  if (map == nullptr || map->name().empty()) return false;
  uint64_t image_base = 0;
  maps->ForEachMapInfo([&](MapInfo* candidate) {
    if (candidate->name() == map->name() && candidate->offset() == 0) {
      image_base = candidate->start();
      return false;
    }
    return true;
  });
  if (image_base == 0 || pc < image_base) return false;

  std::ifstream input(map->name(), std::ios::binary);
  mach_header_64 header{};
  if (!input.read(reinterpret_cast<char*>(&header), sizeof(header)) || header.magic != MH_MAGIC_64 ||
      header.sizeofcmds > 16 * 1024 * 1024) {
    return false;
  }
  std::vector<uint8_t> commands(header.sizeofcmds);
  if (!input.read(reinterpret_cast<char*>(commands.data()), commands.size())) return false;
  symtab_command symbols{};
  uint64_t image_vmaddr = 0;
  size_t command_offset = 0;
  for (uint32_t index = 0; index < header.ncmds && command_offset < commands.size(); ++index) {
    const auto* command = reinterpret_cast<const load_command*>(commands.data() + command_offset);
    if (command->cmdsize < sizeof(load_command) ||
        command_offset + command->cmdsize > commands.size()) {
      return false;
    }
    if (command->cmd == LC_SYMTAB) {
      symbols = *reinterpret_cast<const symtab_command*>(command);
    } else if (command->cmd == LC_SEGMENT_64) {
      const auto* segment = reinterpret_cast<const segment_command_64*>(command);
      if (segment->fileoff == 0) image_vmaddr = segment->vmaddr;
    }
    command_offset += command->cmdsize;
  }
  if (symbols.nsyms == 0 || symbols.strsize == 0) return false;
  std::vector<nlist_64> entries(symbols.nsyms);
  std::vector<char> strings(symbols.strsize);
  input.clear();
  input.seekg(symbols.symoff);
  if (!input.read(reinterpret_cast<char*>(entries.data()), entries.size() * sizeof(nlist_64))) {
    return false;
  }
  input.clear();
  input.seekg(symbols.stroff);
  if (!input.read(strings.data(), strings.size())) return false;

  const uint64_t target = image_vmaddr + (pc - image_base);
  const nlist_64* best = nullptr;
  for (const auto& symbol : entries) {
    if ((symbol.n_type & N_STAB) != 0 || (symbol.n_type & N_TYPE) != N_SECT ||
        symbol.n_value == 0 || symbol.n_value > target || symbol.n_un.n_strx >= strings.size()) {
      continue;
    }
    const char* candidate = strings.data() + symbol.n_un.n_strx;
    const size_t remaining = strings.size() - symbol.n_un.n_strx;
    if (strnlen(candidate, remaining) == remaining || candidate[0] == '\0') continue;
    if (best == nullptr || symbol.n_value > best->n_value) best = &symbol;
  }
  if (best == nullptr) return false;
  const char* raw = strings.data() + best->n_un.n_strx;
  const char* linker_name = raw[0] == '_' ? raw + 1 : raw;
  int demangle_status = 0;
  char* demangled = abi::__cxa_demangle(linker_name, nullptr, nullptr, &demangle_status);
  name->assign(demangle_status == 0 && demangled != nullptr ? demangled : linker_name);
  std::free(demangled);
  *function_offset = target - best->n_value;
  return true;
}

_Unwind_Reason_Code CollectNativeFrame(_Unwind_Context* context, void* opaque) {
  auto* walk = static_cast<NativeWalk*>(opaque);
  if (walk->data->frames.size() >= walk->limit) return _URC_END_OF_STACK;
  const uint64_t pc = _Unwind_GetIP(context);
  if (pc == 0) return _URC_END_OF_STACK;
  walk->last_cfa = _Unwind_GetCFA(context);
  walk->last_x28 = _Unwind_GetGR(context, 28);
  for (size_t reg = 0; reg < walk->last_registers.size(); ++reg) {
    walk->last_registers[reg] = _Unwind_GetGR(context, reg);
  }

  Dl_info symbol{};
  LookupNativeSymbol(walk->maps, pc, &symbol);
  FrameData frame{};
  frame.num = walk->data->frames.size();
  frame.pc = pc;
  frame.sp = _Unwind_GetCFA(context);
  frame.map_info = walk->maps == nullptr ? nullptr : walk->maps->Find(pc);
  if (frame.map_info != nullptr) {
    frame.rel_pc = pc - frame.map_info->start() + frame.map_info->offset();
  }
  if (symbol.dli_sname != nullptr) {
    frame.function_name = symbol.dli_sname;
    frame.function_offset = pc - reinterpret_cast<uint64_t>(symbol.dli_saddr);
  } else {
    // JNI test and application entrypoints may be local (non-exported)
    // Mach-O symbols, for which dladdr returns no name. Use the provider's
    // nlist lookup before treating the frame as managed/OAT code.
    std::string macho_name;
    uint64_t macho_offset = 0;
    if (LookupMachOSymbol(walk->maps, pc, &macho_name, &macho_offset)) {
      frame.function_name = std::move(macho_name);
      frame.function_offset = macho_offset;
      walk->data->frames.emplace_back(std::move(frame));
      return _URC_NO_REASON;
    }
    // AOT app code is an ELF/ODEX mapping, not a Mach-O image and not a JIT
    // descriptor entry. Mirror AOSP Unwinder's normal MapInfo lookup before
    // trying the JIT list so DWARF/symtab method names are recovered directly
    // from the mapped OAT file.
    std::string compiled_name;
    bool found = FindCompiledMethodName(pc, &compiled_name);
    if (found) frame.function_name = compiled_name;
    if (!found) found = frame.map_info != nullptr &&
                 frame.map_info->GetFunctionName(pc, &frame.function_name,
                                                   &frame.function_offset);
    if (!found && walk->jit_debug != nullptr) {
      found = walk->jit_debug->GetFunctionName(
          walk->maps, pc, &frame.function_name, &frame.function_offset);
    }
    if (!found && std::getenv("DARWIN_ART_DEBUG_CFI") != nullptr) {
      std::fprintf(stderr, "darwin-cfi: symbol-miss pc=%llx\\n",
                   static_cast<unsigned long long>(pc));
    }
  }
  walk->data->frames.emplace_back(std::move(frame));
  return _URC_NO_REASON;
}

void AppendManagedFrames(NativeWalk* walk) {
  const bool debug_cfi = std::getenv("DARWIN_ART_DEBUG_CFI") != nullptr;
  auto cfi_debug = [&](const char* message) {
    if (debug_cfi) std::fprintf(stderr, "darwin-cfi: %s\\n", message);
  };
  using ManagedFrameWalker = void (*)(void (*)(const char*, void*), void*);
  auto walker = reinterpret_cast<ManagedFrameWalker>(
      dlsym(RTLD_DEFAULT, "darwin_art_walk_managed_frames"));
  if (walker != nullptr && walk->data->frames.size() < walk->limit) {
    walker(
        [](const char* name, void* context) {
          auto* target = static_cast<NativeWalk*>(context);
          if (name == nullptr || *name == '\0' || target->data->frames.size() >= target->limit) return;
          FrameData frame{};
          frame.num = target->data->frames.size();
          frame.function_name = name;
          target->data->frames.emplace_back(std::move(frame));
        },
        walk);
  }
  if (walk->jit_debug == nullptr || walk->data->frames.empty() ||
      walk->data->frames.size() >= walk->limit) {
    cfi_debug("jit debug/frames/limit rejected");
    return;
  }
  if (!walk->has_registered_quick_frame) {
    const auto& native_caller = walk->data->frames.back();
    if (static_cast<std::string_view>(native_caller.function_name)
            .find("art_quick_generic_jni_trampoline") == std::string_view::npos) {
      cfi_debug("no generic jni frame and no registry");
      return;
    }
  }

  // AOSP's ARM64 generic JNI trampoline keeps the managed SaveRefsAndArgs
  // frame base in x28 while native code uses a separately allocated call
  // frame. Compiled-JNI uses the JNI compiler's fixed 176-byte frame and
  // publishes its untagged SP separately.
  constexpr uint64_t kSaveRefsAndArgsFrameSize = 224;
  const uint64_t frame_size = walk->registered_frame_kind == 1
      ? walk->registered_frame_size : kSaveRefsAndArgsFrameSize;
  if (walk->registered_frame_kind == 1 &&
      (frame_size == 0 || (frame_size & 15u) != 0 || frame_size > 4096 ||
       walk->registered_core_spill_mask == 0)) {
    cfi_debug("compiled frame metadata rejected");
    return;
  }
  if (walk->registered_managed_sp != 0) {
    // libunwind's CFI can describe x28 as undefined after the native callback
    // even though the JNI trampoline published the authoritative managed
    // frame. Never replace that publication with a recovered register value.
    walk->last_x28 = walk->registered_managed_sp;
  }
  if (walk->last_x28 == 0) return;
  std::array<uint64_t, ARM64_REG_R30 - ARM64_REG_R20 + 1> saved_registers{};
  mach_vm_size_t copied = 0;
  const uint64_t managed_return_slot = walk->last_x28 + frame_size - sizeof(uint64_t);
  const uint64_t managed_saved_registers = walk->registered_frame_kind == 1
      ? walk->last_x28 + frame_size -
          static_cast<uint64_t>(__builtin_popcountll(walk->registered_core_spill_mask)) *
              sizeof(uint64_t) + sizeof(uint64_t)
      : managed_return_slot - (saved_registers.size() - 1) * sizeof(uint64_t);
  if (mach_vm_read_overwrite(walk->task, managed_saved_registers,
                             sizeof(saved_registers),
                             reinterpret_cast<mach_vm_address_t>(saved_registers.data()),
                             &copied) !=
          KERN_SUCCESS ||
      copied != sizeof(saved_registers)) {
    cfi_debug("managed register read failed");
    return;
  }
  uint64_t return_pc = saved_registers.back();
  return_pc = StripReturnAddress(return_pc);
  return_pc = NormalizeManagedPc(walk->maps, return_pc);
  if (return_pc == 0) {
    cfi_debug("managed return pc is zero");
    return;
  }
  if (debug_cfi) {
    std::fprintf(stderr, "darwin-cfi: kind=%llu sp=%p size=%llu mask=%llx return=%llx\\n",
                 static_cast<unsigned long long>(walk->registered_frame_kind),
                 reinterpret_cast<void*>(walk->last_x28),
                 static_cast<unsigned long long>(frame_size),
                 static_cast<unsigned long long>(walk->registered_core_spill_mask),
                 static_cast<unsigned long long>(return_pc));
    uint64_t words[4]{};
    mach_vm_size_t word_bytes = 0;
    if (mach_vm_read_overwrite(walk->task, walk->last_x28, sizeof(words),
                               reinterpret_cast<mach_vm_address_t>(words), &word_bytes) == KERN_SUCCESS) {
      std::fprintf(stderr, "darwin-cfi: frame words=%llx,%llx,%llx,%llx\\n",
                   static_cast<unsigned long long>(words[0]),
                   static_cast<unsigned long long>(words[1]),
                   static_cast<unsigned long long>(words[2]),
                   static_cast<unsigned long long>(words[3]));
    }
    uint64_t frame_tail[4]{};
    mach_vm_size_t tail_bytes = 0;
    if (mach_vm_read_overwrite(walk->task, walk->last_x28 + 192, sizeof(frame_tail),
                               reinterpret_cast<mach_vm_address_t>(frame_tail), &tail_bytes) == KERN_SUCCESS) {
      std::fprintf(stderr, "darwin-cfi: frame tail x27=%llx x28=%llx x29=%llx lr=%llx\\n",
                   static_cast<unsigned long long>(frame_tail[0]),
                   static_cast<unsigned long long>(frame_tail[1]),
                   static_cast<unsigned long long>(frame_tail[2]),
                   static_cast<unsigned long long>(frame_tail[3]));
    }
  }

  RegsArm64 regs;
  auto* raw = static_cast<uint64_t*>(regs.RawData());
  for (size_t reg = 0; reg < walk->last_registers.size(); ++reg) {
    raw[reg] = walk->last_registers[reg];
  }
  for (size_t index = 0; index < saved_registers.size(); ++index) {
    raw[ARM64_REG_R20 + index] = saved_registers[index];
  }
  // The managed unwinder must advance over the frame that was actually
  // published.  Generic JNI uses SaveRefsAndArgs (224 bytes), while compiled
  // JNI uses the AArch64 JNI compiler's 176-byte frame; using the former for
  // both skips the caller's saved-register boundary in compiled JNI.
  regs.set_sp(walk->last_x28 + frame_size);
  // A saved LR points immediately after the call. Use the call-site PC so the
  // JIT FDE and inline-info lookup select the caller's instruction range.
  regs.set_pc(return_pc - 1);

  Unwinder managed(walk->limit - walk->data->frames.size(), walk->maps, &regs, walk->memory);
  managed.SetJitDebug(walk->jit_debug);
  managed.SetDexFiles(walk->dex_files);
  managed.Unwind();
  auto frames = managed.ConsumeFrames();
  for (auto& frame : frames) {
    if (frame.pc < (1ULL << 32)) {
      const uint64_t logical_pc = frame.pc;
      const uint64_t normalized_pc = NormalizeManagedPc(walk->maps, logical_pc);
      if (normalized_pc != frame.pc) {
        frame.pc = normalized_pc;
        frame.map_info = walk->maps->Find(normalized_pc);
        if (frame.map_info != nullptr) {
          frame.rel_pc = normalized_pc - frame.map_info->start() + frame.map_info->offset();
        }
        SharedString resolved_name;
        uint64_t resolved_offset = 0;
        const bool jit_found = walk->jit_debug->GetFunctionName(
            walk->maps, normalized_pc, &resolved_name, &resolved_offset);
        if (jit_found) {
          frame.function_name = resolved_name;
          frame.function_offset = resolved_offset;
        } else {
          bool dex_found = walk->dex_files != nullptr &&
                           walk->dex_files->GetFunctionName(
                               walk->maps, normalized_pc, &resolved_name, &resolved_offset);
          // DexFiles is keyed by the logical DEX PC.  The normalized host PC
          // is useful for map lookup but is not a valid DEX lookup key.
          if (!dex_found && walk->dex_files != nullptr) {
            dex_found = walk->dex_files->GetFunctionName(
                walk->maps, logical_pc, &resolved_name, &resolved_offset);
          }
          if (std::getenv("DARWIN_ART_DEBUG_CFI") != nullptr) {
            std::fprintf(stderr, "darwin-cfi: frame-symbol pc=%llx jit=%d dex=%d dex_obj=%p\\n",
                         static_cast<unsigned long long>(normalized_pc), jit_found, dex_found,
                         static_cast<const void*>(walk->dex_files));
          }
          if (dex_found) {
            frame.function_name = resolved_name;
            frame.function_offset = resolved_offset;
          }
        }
      }
    }
    // Unwindstack may already contain a host-window PC, in which case the
    // legacy low-PC branch above is skipped. Recover Android's logical PC and
    // give DexFiles the same lookup opportunity regardless of normalization.
    if (walk->dex_files != nullptr &&
        static_cast<std::string_view>(frame.function_name).empty() &&
        frame.pc >= kDarwinArtCompressedReferenceBase &&
        frame.pc - kDarwinArtCompressedReferenceBase < (1ULL << 32)) {
      const uint64_t logical_pc = frame.pc - kDarwinArtCompressedReferenceBase;
      SharedString resolved_name;
      uint64_t resolved_offset = 0;
      if (walk->dex_files->GetFunctionName(walk->maps, logical_pc, &resolved_name,
                                           &resolved_offset)) {
        frame.function_name = resolved_name;
        frame.function_offset = resolved_offset;
      }
    }
    frame.num = walk->data->frames.size();
    walk->data->frames.emplace_back(std::move(frame));
  }
}

void AppendFrame(NativeWalk* walk, uint64_t pc, uint64_t sp) {
  Dl_info symbol{};
  std::string mach_function_name;
  uint64_t mach_function_offset = 0;
  const bool remote_mach_symbol =
      walk->task != mach_task_self() &&
      LookupMachOSymbol(walk->maps, pc, &mach_function_name, &mach_function_offset);
  if (!remote_mach_symbol) LookupNativeSymbol(walk->maps, pc, &symbol);
  FrameData frame{};
  frame.num = walk->data->frames.size();
  frame.pc = pc;
  frame.sp = sp;
  frame.map_info = walk->maps == nullptr ? nullptr : walk->maps->Find(pc);
  if (frame.map_info != nullptr) {
    frame.rel_pc = pc - frame.map_info->start() + frame.map_info->offset();
  }
  if (remote_mach_symbol) {
    frame.function_name = std::move(mach_function_name);
    frame.function_offset = mach_function_offset;
  } else if (symbol.dli_sname != nullptr) {
    frame.function_name = symbol.dli_sname;
    frame.function_offset = pc - reinterpret_cast<uint64_t>(symbol.dli_saddr);
  } else if (walk->jit_debug != nullptr) {
    walk->jit_debug->GetFunctionName(walk->maps, pc, &frame.function_name,
                                     &frame.function_offset);
  }
  walk->data->frames.emplace_back(std::move(frame));
}

bool CollectCursor(unw_cursor_t* cursor, NativeWalk* walk) {
  while (walk->data->frames.size() < walk->limit) {
    unw_word_t pc = 0;
    unw_word_t sp = 0;
    if (unw_get_reg(cursor, UNW_REG_IP, &pc) != UNW_ESUCCESS ||
        unw_get_reg(cursor, UNW_REG_SP, &sp) != UNW_ESUCCESS || pc == 0) {
      break;
    }
    AppendFrame(walk, pc, sp);
    if (unw_step(cursor) <= 0) break;
  }
  return !walk->data->frames.empty();
}

uint64_t StripReturnAddress(uint64_t address) {
#if __has_feature(ptrauth_calls)
  return reinterpret_cast<uint64_t>(ptrauth_strip(reinterpret_cast<void*>(address),
                                                  ptrauth_key_return_address));
#else
  return address;
#endif
}

bool ReadFrameRecord(mach_port_t task, uint64_t address, uint64_t (&record)[2]) {
  mach_vm_size_t copied = 0;
  return mach_vm_read_overwrite(task, address, sizeof(record),
                                reinterpret_cast<mach_vm_address_t>(record),
                                &copied) == KERN_SUCCESS &&
         copied == sizeof(record);
}

// DarwinNativeUnwind runs on the target thread itself. Its frame pointer is
// therefore an address in the current, already mapped stack; avoid a Mach
// VM round-trip on every GC-stress allocation. Remote/thread-targeted walks
// continue to use ReadFrameRecord above, which retains the fault-safe read.
bool ReadLocalFrameRecord(uint64_t address, uint64_t (&record)[2]) {
  if (address == 0 || (address & (alignof(uint64_t) - 1)) != 0) return false;
  std::memcpy(record, reinterpret_cast<const void*>(address), sizeof(record));
  return true;
}

struct RegisteredQuickFrame {
  uint64_t managed_sp = 0;
  uint64_t frame_kind = 0;
  uint64_t frame_size = 0;
  uint64_t core_spill_mask = 0;
  bool is_main_thread = false;
};

RegisteredQuickFrame ReadRemoteQuickFrame(mach_port_t task, uint64_t registry_address,
                                          uint64_t thread_id) {
  if (registry_address == 0 || thread_id == 0) return {};
  DarwinArtQuickFrameRegistry registry{};
  mach_vm_size_t copied = 0;
  if (mach_vm_read_overwrite(task, registry_address, sizeof(registry),
                             reinterpret_cast<mach_vm_address_t>(&registry), &copied) !=
          KERN_SUCCESS ||
      copied != sizeof(registry) || registry.version != kDarwinArtQuickFrameRegistryVersion ||
      registry.slot_count != kDarwinArtQuickFrameRegistrySlots) {
    return {};
  }
  for (const auto& slot : registry.slots) {
    if (slot.thread_id != thread_id || slot.depth == 0 ||
        slot.depth > kDarwinArtQuickFrameRegistryDepth) {
      continue;
    }
    return {slot.frames[slot.depth - 1], slot.frame_kinds[slot.depth - 1],
            slot.frame_sizes[slot.depth - 1], slot.core_spill_masks[slot.depth - 1],
            registry.art_main_thread_id == thread_id};
  }
  return {};
}

uint64_t ThreadIdentifier(thread_t thread) {
  thread_identifier_info_data_t info{};
  mach_msg_type_number_t count = THREAD_IDENTIFIER_INFO_COUNT;
  if (thread_info(thread, THREAD_IDENTIFIER_INFO, reinterpret_cast<thread_info_t>(&info),
                  &count) != KERN_SUCCESS) {
    return 0;
  }
  return info.thread_id;
}

bool CollectFrameRecords(mach_port_t task, uint64_t pc, uint64_t sp, uint64_t fp,
                         NativeWalk* walk) {
  walk->data->frames.clear();
  AppendFrame(walk, pc, sp);
  while (fp != 0 && walk->data->frames.size() < walk->limit) {
    if ((fp & (alignof(uint64_t) - 1)) != 0 || fp < sp || fp - sp > 8 * 1024 * 1024) break;
    uint64_t record[2]{};
    if (!ReadFrameRecord(task, fp, record)) break;
    const uint64_t next_fp = record[0];
    const uint64_t return_pc = StripReturnAddress(record[1]);
    if (return_pc == 0 || next_fp <= fp) break;
    AppendFrame(walk, return_pc - 1, fp + sizeof(record));
    sp = fp + sizeof(record);
    fp = next_fp;
    if (static_cast<std::string_view>(walk->data->frames.back().function_name)
            .find("art_quick_generic_jni_trampoline") != std::string_view::npos) {
      break;
    }
  }
  return !walk->data->frames.empty();
}

thread_t FindThread(mach_port_t task, uint64_t thread_id) {
  thread_act_array_t threads = nullptr;
  mach_msg_type_number_t count = 0;
  if (task_threads(task, &threads, &count) != KERN_SUCCESS) return MACH_PORT_NULL;
  thread_t found = MACH_PORT_NULL;
  for (mach_msg_type_number_t index = 0; index < count; ++index) {
    thread_identifier_info_data_t info{};
    mach_msg_type_number_t info_count = THREAD_IDENTIFIER_INFO_COUNT;
    if (thread_info(threads[index], THREAD_IDENTIFIER_INFO,
                    reinterpret_cast<thread_info_t>(&info), &info_count) == KERN_SUCCESS &&
        (thread_id == 0 || info.thread_id == thread_id)) {
      found = threads[index];
      threads[index] = MACH_PORT_NULL;
      break;
    }
  }
  for (mach_msg_type_number_t index = 0; index < count; ++index) {
    if (threads[index] != MACH_PORT_NULL) mach_port_deallocate(mach_task_self(), threads[index]);
  }
  vm_deallocate(mach_task_self(), reinterpret_cast<vm_address_t>(threads),
                count * sizeof(thread_t));
  return found;
}

}  // namespace

uint64_t DarwinFindGlobalVariable(Maps* maps, const char* variable) {
  if (maps == nullptr || variable == nullptr) return 0;
  const std::string mach_symbol = std::string("_") + variable;
  const bool require_nonempty_jit_descriptor =
      std::string_view(variable) == "__jit_debug_descriptor";
  std::unordered_set<std::string> visited;
  uint64_t result = 0;
  maps->ForEachMapInfo([&](MapInfo* map) {
    if (result != 0 || map->offset() != 0 || map->name().empty() ||
        !visited.emplace(map->name()).second) {
      return result == 0;
    }
    std::ifstream input(map->name(), std::ios::binary);
    mach_header_64 header{};
    if (!input.read(reinterpret_cast<char*>(&header), sizeof(header)) ||
        header.magic != MH_MAGIC_64 || header.sizeofcmds > 16 * 1024 * 1024) {
      return true;
    }
    std::vector<uint8_t> commands(header.sizeofcmds);
    if (!input.read(reinterpret_cast<char*>(commands.data()), commands.size())) return true;
    symtab_command symbols{};
    uint64_t image_vmaddr = 0;
    size_t offset = 0;
    for (uint32_t index = 0; index < header.ncmds && offset < commands.size(); ++index) {
      const auto* command = reinterpret_cast<const load_command*>(commands.data() + offset);
      if (command->cmdsize < sizeof(load_command) || offset + command->cmdsize > commands.size()) {
        return true;
      }
      if (command->cmd == LC_SYMTAB) {
        symbols = *reinterpret_cast<const symtab_command*>(command);
      } else if (command->cmd == LC_SEGMENT_64) {
        const auto* segment = reinterpret_cast<const segment_command_64*>(command);
        if (segment->fileoff == 0) image_vmaddr = segment->vmaddr;
      }
      offset += command->cmdsize;
    }
    if (symbols.nsyms == 0 || symbols.strsize == 0 || map->start() < image_vmaddr) return true;
    std::vector<nlist_64> entries(symbols.nsyms);
    std::vector<char> strings(symbols.strsize);
    input.clear();
    input.seekg(symbols.symoff);
    if (!input.read(reinterpret_cast<char*>(entries.data()),
                    entries.size() * sizeof(nlist_64))) {
      return true;
    }
    input.clear();
    input.seekg(symbols.stroff);
    if (!input.read(strings.data(), strings.size())) return true;
    const uint64_t slide = map->start() - image_vmaddr;
    for (const auto& symbol : entries) {
      if (symbol.n_un.n_strx >= strings.size() || symbol.n_value == 0) continue;
      const char* name = strings.data() + symbol.n_un.n_strx;
      const size_t remaining = strings.size() - symbol.n_un.n_strx;
      if (strnlen(name, remaining) == remaining) continue;
      if (mach_symbol == name) {
        if (require_nonempty_jit_descriptor) {
          // This provider may be linked into more than one Mach-O image. A
          // probe image can therefore expose an empty descriptor before the
          // ART runtime's live JIT descriptor. The first-entry field is the
          // stable v1/v2 discriminator; keep searching until it is populated.
          uint64_t first_entry = 0;
          mach_vm_size_t copied = 0;
          if (mach_vm_read_overwrite(mach_task_self(), symbol.n_value + slide + 16,
                                     sizeof(first_entry),
                                     reinterpret_cast<mach_vm_address_t>(&first_entry),
                                     &copied) != KERN_SUCCESS ||
              copied != sizeof(first_entry) || first_entry == 0) {
            if (std::getenv("DARWIN_ART_DEBUG_CFI") != nullptr) {
              std::fprintf(stderr,
                           "darwin-cfi: jit-descriptor candidate=%llx first=%llx empty=1\\n",
                           static_cast<unsigned long long>(symbol.n_value + slide),
                           static_cast<unsigned long long>(first_entry));
            }
            continue;
          }
          if (std::getenv("DARWIN_ART_DEBUG_CFI") != nullptr) {
            std::fprintf(stderr,
                         "darwin-cfi: jit-descriptor candidate=%llx first=%llx empty=0\\n",
                         static_cast<unsigned long long>(symbol.n_value + slide),
                         static_cast<unsigned long long>(first_entry));
            uint64_t entry_words[4]{};
            mach_vm_size_t entry_bytes = 0;
            if (mach_vm_read_overwrite(mach_task_self(), first_entry,
                                       sizeof(entry_words),
                                       reinterpret_cast<mach_vm_address_t>(entry_words),
                                       &entry_bytes) == KERN_SUCCESS &&
                entry_bytes == sizeof(entry_words)) {
              std::fprintf(stderr,
                           "darwin-cfi: jit-entry next=%llx prev=%llx symfile=%llx size=%llx\\n",
                           static_cast<unsigned long long>(entry_words[0]),
                           static_cast<unsigned long long>(entry_words[1]),
                           static_cast<unsigned long long>(entry_words[2]),
                           static_cast<unsigned long long>(entry_words[3]));
            }
          }
        }
        result = symbol.n_value + slide;
        return false;
      }
    }
    return true;
  });
  return result;
}

bool DarwinNativeUnwind(Maps* maps, JitDebug* jit_debug, DexFiles* dex_files, size_t max_frames,
                        AndroidUnwinderData& data) {
  data.frames.clear();
  data.error = {ERROR_NONE, 0};
  const size_t limit = data.max_frames.value_or(max_frames);
  // Resolve the caller and managed-frame identity before constructing the
  // unwind walk. GC-stress requests a backtrace for every allocation; once the
  // same managed frame repeats, the cache hit must avoid NativeWalk setup,
  // map publication, and Mach-backed memory initialization entirely.
  const uint64_t frame_pointer = reinterpret_cast<uint64_t>(__builtin_frame_address(0));
  uint64_t caller_record[2]{};
  if (!ReadLocalFrameRecord(frame_pointer, caller_record)) {
    data.error.code = ERROR_MEMORY_INVALID;
    return false;
  }
  const uint64_t caller_pc = StripReturnAddress(caller_record[1]);
  uint64_t registered_managed_sp = 0;
  uint64_t registered_frame_kind = 0;
  uint64_t registered_frame_size = 0;
  uint64_t registered_core_spill_mask = 0;
  uint64_t thread_id = 0;
  const bool have_thread_id = pthread_threadid_np(nullptr, &thread_id) == 0 && thread_id != 0;
  const bool has_registered_quick_frame =
      have_thread_id && ReadLocalQuickFrame(thread_id, &registered_managed_sp, &registered_frame_kind,
                                            &registered_frame_size, &registered_core_spill_mask);
  if (!has_registered_quick_frame) {
    uint64_t generic_jni_sp = 0;
    if (android::CurrentGenericJniFrame != nullptr &&
        android::CurrentGenericJniFrame(&generic_jni_sp)) {
      registered_managed_sp = generic_jni_sp;
    }
  }
  uint64_t shadow_frame = 0;
  const bool has_shadow_frame = android::CurrentInterpreterFrame(&shadow_frame);
  const bool cache_key_matches =
      g_local_managed_backtrace_cache.caller_pc == caller_pc &&
      !g_local_managed_backtrace_cache.frames.empty() &&
      ((registered_managed_sp != 0 &&
        g_local_managed_backtrace_cache.managed_sp == registered_managed_sp) ||
       (registered_managed_sp == 0 && has_shadow_frame &&
        g_local_managed_backtrace_cache.shadow_frame == shadow_frame) ||
       (registered_managed_sp == 0 && !has_shadow_frame &&
        g_local_managed_backtrace_cache.frame_pointer == frame_pointer));
  if (cache_key_matches) {
    data.frames = g_local_managed_backtrace_cache.frames;
    if (data.frames.size() > limit) data.frames.resize(limit);
    return true;
  }

  DarwinPublishAotCodeMaps(maps);
  NativeWalk walk{maps, jit_debug, dex_files, &data, limit, mach_task_self(),
                  Memory::CreateProcessMemoryThreadCached(getpid())};
  // The generic-JNI trampoline deliberately leaves no unwindable native frame
  // between the JNI entry and the managed caller.  It publishes the managed
  // SaveRefsAndArgs frame in the same registry used by the Mach remote path;
  // consume that publication for local unwinds as well.  Relying on the last
  // native symbol name is insufficient because libunwind may stop at the
  // native callback before visiting the trampoline's CFI range.
  if (have_thread_id) {
    if (has_registered_quick_frame) {
      walk.registered_managed_sp = registered_managed_sp;
      walk.registered_frame_kind = registered_frame_kind;
      walk.registered_frame_size = registered_frame_size;
      walk.registered_core_spill_mask = registered_core_spill_mask;
      walk.last_x28 = walk.registered_managed_sp;
      walk.has_registered_quick_frame = true;
    }
  }
  // Generic-JNI transitions intentionally do not publish the compiled-frame
  // registry: the ManagedStack top is tagged instead. Recover that AOSP
  // SaveRefsAndArgs frame directly for local unwinds so a native callback can
  // continue through the managed caller without relying on a host trampoline.
  if (!walk.has_registered_quick_frame) {
    if (registered_managed_sp != 0) {
      walk.registered_managed_sp = registered_managed_sp;
      walk.registered_frame_kind = 0;
      walk.registered_frame_size = 224;
      walk.registered_core_spill_mask = 0;
      walk.last_x28 = walk.registered_managed_sp;
      walk.has_registered_quick_frame = true;
    }
  }
  // Apple's libunwind authenticates each saved LR while stepping. ART's
  // ARM64 quick and Nterp frames intentionally use Android's unsigned return
  // address ABI, so `_Unwind_Backtrace` traps in libunwind when a native
  // allocation stack reaches one of those frames (GC stress does this before
  // managed main). Walk Darwin's ordinary frame records just as the remote
  // Mach-task path does and strip, rather than authenticate, saved return
  // addresses at the host boundary.
  const bool collected = caller_pc != 0 && CollectFrameRecords(
      mach_task_self(), caller_pc - 1, frame_pointer + sizeof(caller_record),
      caller_record[0], &walk);
  if (collected) {
    AppendManagedFrames(&walk);
    if (caller_pc != 0) {
      g_local_managed_backtrace_cache.caller_pc = caller_pc;
      g_local_managed_backtrace_cache.frame_pointer = frame_pointer;
      g_local_managed_backtrace_cache.managed_sp = walk.registered_managed_sp;
      g_local_managed_backtrace_cache.shadow_frame = has_shadow_frame ? shadow_frame : 0;
      g_local_managed_backtrace_cache.frames = data.frames;
    }
  }
  return collected;
}

bool DarwinNativeUnwindUcontext(Maps* maps, JitDebug* jit_debug, DexFiles* dex_files, size_t max_frames, void* ucontext,
                                AndroidUnwinderData& data) {
  DarwinPublishAotCodeMaps(maps);
  data.frames.clear();
  if (ucontext == nullptr) {
    data.error = {ERROR_INVALID_PARAMETER, 0};
    return false;
  }
  data.error = {ERROR_NONE, 0};
  auto* context = static_cast<ucontext_t*>(ucontext);
  if (context->uc_mcontext == nullptr) {
    data.error.code = ERROR_INVALID_PARAMETER;
    return false;
  }
  const auto& state = context->uc_mcontext->__ss;
  NativeWalk walk{maps, jit_debug, dex_files, &data, data.max_frames.value_or(max_frames), mach_task_self(),
                  Memory::CreateProcessMemoryThreadCached(getpid())};
  // Ucontext unwinds are used by the in-process CFI probe and arrive on the
  // ART thread itself.  The generic-JNI transition may not publish the
  // auxiliary quick-frame registry, so recover the authoritative AOSP
  // ManagedStack top before collecting native frames.
  uint64_t generic_jni_sp = 0;
  if (android::CurrentGenericJniFrame != nullptr &&
      android::CurrentGenericJniFrame(&generic_jni_sp)) {
    walk.registered_managed_sp = generic_jni_sp;
    walk.registered_frame_kind = 0;
    walk.registered_frame_size = 224;
    walk.registered_core_spill_mask = 0;
    walk.last_x28 = generic_jni_sp;
    walk.has_registered_quick_frame = true;
  }
  return CollectFrameRecords(mach_task_self(), state.__pc, state.__sp, state.__fp, &walk);
}

bool DarwinNativeUnwindThread(Maps* maps, JitDebug* jit_debug, DexFiles* dex_files, size_t max_frames,
                              uint64_t thread_id,
                              AndroidUnwinderData& data) {
  DarwinPublishAotCodeMaps(maps);
  data.frames.clear();
  data.error = {ERROR_NONE, 0};
  thread_t thread = FindThread(mach_task_self(), thread_id);
  if (thread == MACH_PORT_NULL) {
    data.error.code = ERROR_THREAD_DOES_NOT_EXIST;
    return false;
  }
  if (thread_suspend(thread) != KERN_SUCCESS) {
    mach_port_deallocate(mach_task_self(), thread);
    data.error.code = ERROR_SYSTEM_CALL;
    return false;
  }
  arm_thread_state64_t state{};
  mach_msg_type_number_t count = ARM_THREAD_STATE64_COUNT;
  const kern_return_t state_status = thread_get_state(
      thread, ARM_THREAD_STATE64, reinterpret_cast<thread_state_t>(&state), &count);
  uint64_t registered_managed_sp = 0;
  uint64_t registered_frame_kind = 0;
  uint64_t registered_frame_size = 0;
  uint64_t registered_core_spill_mask = 0;
  const bool has_registered_quick_frame =
      state_status == KERN_SUCCESS &&
      ReadLocalQuickFrame(thread_id, &registered_managed_sp, &registered_frame_kind,
                          &registered_frame_size, &registered_core_spill_mask);
  ResumeThreadFully(thread);
  mach_port_deallocate(mach_task_self(), thread);
  if (state_status != KERN_SUCCESS) {
    data.error.code = ERROR_SYSTEM_CALL;
    return false;
  }

  unw_context_t context{};
  unw_cursor_t cursor{};
  if (unw_getcontext(&context) != UNW_ESUCCESS ||
      unw_init_local(&cursor, &context) != UNW_ESUCCESS) {
    data.error.code = ERROR_UNSUPPORTED;
    return false;
  }
  for (int index = 0; index < 29; ++index) {
    unw_set_reg(&cursor, UNW_ARM64_X0 + index, state.__x[index]);
  }
  unw_set_reg(&cursor, UNW_ARM64_FP, state.__fp);
  unw_set_reg(&cursor, UNW_ARM64_LR, state.__lr);
  unw_set_reg(&cursor, UNW_REG_SP, state.__sp);
  unw_set_reg(&cursor, UNW_REG_IP, state.__pc);
  NativeWalk walk{maps, jit_debug, dex_files, &data, data.max_frames.value_or(max_frames), mach_task_self(),
                  Memory::CreateProcessMemoryThreadCached(getpid())};
  walk.registered_managed_sp = registered_managed_sp;
  walk.registered_frame_kind = registered_frame_kind;
  walk.registered_frame_size = registered_frame_size;
  walk.registered_core_spill_mask = registered_core_spill_mask;
  walk.last_x28 = walk.registered_managed_sp;
  walk.has_registered_quick_frame = has_registered_quick_frame;
  if (CollectCursor(&cursor, &walk) && data.frames.size() > 1) {
    AppendManagedFrames(&walk);
    return true;
  }
  const bool collected =
      CollectFrameRecords(mach_task_self(), state.__pc, state.__sp, state.__fp, &walk);
  if (collected) AppendManagedFrames(&walk);
  return collected;
}

bool DarwinNativeUnwindRemote(Maps* maps, JitDebug* jit_debug, DexFiles* dex_files, size_t max_frames, int process_id,
                              uint64_t thread_id,
                              AndroidUnwinderData& data) {
  DarwinPublishAotCodeMaps(maps);
  data.frames.clear();
  data.error = {ERROR_NONE, 0};
  mach_port_t task = darwin_art::remote_task::Acquire(process_id);
  if (task == MACH_PORT_NULL) {
    // A task port is a privileged Darwin capability. Report the same
    // transport-level failure category that AOSP's ptrace backend exposes so
    // callers can distinguish host access denial from a malformed unwind.
    data.error.code = ERROR_PTRACE_CALL;
    return false;
  }
  auto memory = Memory::CreateProcessMemoryCached(process_id);
  const uint64_t quick_frame_registry =
      DarwinFindGlobalVariable(maps, "darwin_art_unwindstack_quick_frames");
  auto collect = [&](thread_t thread, AndroidUnwinderData* output, bool* is_main_thread) {
    if (thread_suspend(thread) != KERN_SUCCESS) return false;
    arm_thread_state64_t state{};
    mach_msg_type_number_t count = ARM_THREAD_STATE64_COUNT;
    const kern_return_t state_status = thread_get_state(
        thread, ARM_THREAD_STATE64, reinterpret_cast<thread_state_t>(&state), &count);
    // Remote collection owns one suspend level per sample; do not loop here,
    // because a second resume can race the target task's own state transition.
    thread_resume(thread);
    if (state_status != KERN_SUCCESS) return false;
    NativeWalk walk{maps, jit_debug, dex_files, output, output->max_frames.value_or(max_frames), task, memory};
    for (size_t reg = 0; reg < 29; ++reg) walk.last_registers[reg] = state.__x[reg];
    walk.last_registers[ARM64_REG_R29] = state.__fp;
    walk.last_registers[ARM64_REG_R30] = state.__lr;
    const RegisteredQuickFrame registered_quick_frame =
        ReadRemoteQuickFrame(task, quick_frame_registry, ThreadIdentifier(thread));
    if (is_main_thread != nullptr) *is_main_thread = registered_quick_frame.is_main_thread;
    walk.last_x28 =
        registered_quick_frame.managed_sp != 0 ? registered_quick_frame.managed_sp : state.__x[28];
    walk.registered_managed_sp = registered_quick_frame.managed_sp;
    walk.registered_frame_kind = registered_quick_frame.frame_kind;
    walk.registered_frame_size = registered_quick_frame.frame_size;
    walk.registered_core_spill_mask = registered_quick_frame.core_spill_mask;
    walk.has_registered_quick_frame = registered_quick_frame.managed_sp != 0;
    const bool success =
        CollectFrameRecords(task, state.__pc, state.__sp, state.__fp, &walk);
    if (success) AppendManagedFrames(&walk);
    return success;
  };

  bool success = false;
  if (thread_id != 0) {
    thread_t thread = FindThread(task, thread_id);
    if (thread != MACH_PORT_NULL) {
      success = collect(thread, &data, nullptr);
      mach_port_deallocate(mach_task_self(), thread);
    }
  } else {
    thread_act_array_t threads = nullptr;
    mach_msg_type_number_t count = 0;
    if (task_threads(task, &threads, &count) == KERN_SUCCESS) {
      AndroidUnwinderData fallback(data.max_frames.value_or(max_frames));
      for (mach_msg_type_number_t index = 0; index < count; ++index) {
        AndroidUnwinderData candidate(data.max_frames.value_or(max_frames));
        bool is_main_thread = false;
        if (!collect(threads[index], &candidate, &is_main_thread)) continue;
        if (is_main_thread) {
          data.frames = std::move(candidate.frames);
          data.error = candidate.error;
          success = true;
          break;
        }
        if (fallback.frames.empty()) {
          fallback.frames = std::move(candidate.frames);
          fallback.error = candidate.error;
        }
      }
      if (!success && !fallback.frames.empty()) {
        data.frames = std::move(fallback.frames);
        data.error = fallback.error;
        success = true;
      }
      for (mach_msg_type_number_t index = 0; index < count; ++index) {
        mach_port_deallocate(mach_task_self(), threads[index]);
      }
      vm_deallocate(mach_task_self(), reinterpret_cast<vm_address_t>(threads),
                    count * sizeof(thread_t));
    }
  }
  if (!success) data.error.code = ERROR_THREAD_DOES_NOT_EXIST;
  mach_port_deallocate(mach_task_self(), task);
  return success;
}

}  // namespace unwindstack
