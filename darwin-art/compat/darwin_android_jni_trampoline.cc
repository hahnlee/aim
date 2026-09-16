#include "darwin_android_jni_trampoline.h"

extern "C" __attribute__((weak)) void darwin_art_unwindstack_push_quick_frame(
    void* managed_sp) {
  (void)managed_sp;
}
extern "C" __attribute__((weak)) void darwin_art_unwindstack_pop_quick_frame() {}

#include <sys/mman.h>
#include <unistd.h>

#include <atomic>
#include <cerrno>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <mutex>
#include <new>
#include <string>
#include <unordered_map>
#include <utility>
#include <vector>
#include <algorithm>

namespace darwin_art::android_jni {
namespace {

constexpr size_t kMaxRequests = 4096;
constexpr size_t kMaxExecutableBytes = 16u * 1024u * 1024u;
constexpr size_t kMaxShortyLength = 256;
constexpr size_t kDarwinSavedFrameSize = 16;
constexpr size_t kInstructionSize = sizeof(uint32_t);
constexpr size_t kLiteralSize = sizeof(uint64_t);

struct PublishedEntry {
  uintptr_t address = 0;
  uint32_t mask = 0;
};

struct RegistryEntry {
  uintptr_t start = 0;
  uintptr_t end = 0;
  uint64_t generation = 0;
  std::vector<PublishedEntry> entries;
};

struct StackMove {
  size_t darwin_offset;
  size_t android_offset;
  size_t size;
};

struct ShortyPlan {
  std::vector<StackMove> moves;
  size_t android_stack_size = 0;
};

enum class CallShape {
  kRegular,
  kCritical,
  kLifecycle,
};

// Private area after the Android stack tail. The unwind callback is ordinary
// C code and may clobber caller-saved argument registers.
constexpr size_t kJniThunkScratchBytes = 224;

// Android libraries commonly register one native table per Java class. Keep
// the executable-range registry proportional to the loaded app instead of
// imposing an artificial per-process class limit.
std::vector<RegistryEntry> g_registry;
std::mutex g_registry_mutex;
std::atomic<uint64_t> g_next_generation{1};
std::atomic<size_t> g_live_count{0};

size_t RoundUp(size_t value, size_t alignment) {
  return (value + alignment - 1u) & ~(alignment - 1u);
}

bool IsReturnType(char type) {
  switch (type) {
    case 'V':
    case 'Z':
    case 'B':
    case 'C':
    case 'S':
    case 'I':
    case 'J':
    case 'F':
    case 'D':
    case 'L':
      return true;
    default:
      return false;
  }
}

bool TypeSize(char type, size_t* size) {
  switch (type) {
    case 'Z':
    case 'B':
      *size = 1;
      return true;
    case 'C':
    case 'S':
      *size = 2;
      return true;
    case 'I':
    case 'F':
      *size = 4;
      return true;
    case 'J':
    case 'D':
    case 'L':
      *size = 8;
      return true;
    default:
      return false;
  }
}

bool PlanShorty(const char* shorty,
                CallShape shape,
                ShortyPlan* plan,
                std::string* error) {
  if (shorty == nullptr) {
    *error = "regular JNI shorty is null";
    return false;
  }
  const size_t length = ::strnlen(shorty, kMaxShortyLength + 1u);
  if (length == 0 || length > kMaxShortyLength || !IsReturnType(shorty[0])) {
    *error = "regular JNI shorty has an invalid return type or length";
    return false;
  }
  if (shape == CallShape::kCritical && shorty[0] == 'L') {
    *error = "CriticalNative shorty cannot return a reference";
    return false;
  }
  size_t gp_count = shape == CallShape::kRegular ? 2 : 0;
  size_t fp_count = 0;
  size_t darwin_offset = 0;
  size_t android_offset = 0;
  for (size_t index = 1; index < length; ++index) {
    const char type = shorty[index];
    if (shape == CallShape::kCritical && type == 'L') {
      *error = "CriticalNative shorty cannot contain reference arguments";
      return false;
    }
    size_t size = 0;
    if (!TypeSize(type, &size)) {
      *error = "regular JNI shorty contains V or a non-scalar argument";
      return false;
    }
    const bool fp = type == 'F' || type == 'D';
    size_t& register_count = fp ? fp_count : gp_count;
    if (register_count < 8) {
      ++register_count;
      continue;
    }
    darwin_offset = RoundUp(darwin_offset, size);
    plan->moves.push_back({darwin_offset, android_offset, size});
    darwin_offset += size;
    android_offset += 8;
  }
  // Keep the Android ABI tail at least one 16-byte pair even when the method
  // has no stack args, then reserve two private scratch slots after it.
  // Keep two private scratch slots after the guest argument tail.  The
  // trampoline must preserve x0 across the unwind-frame callback, but using
  // [sp] for that scratch would overwrite the first Android stack argument.
  const size_t guest_stack_size = RoundUp(std::max(android_offset, size_t{16}), 16);
  // Preserve the historical 16-byte frame for register-only calls.  ART's
  // unwind bridge observes that frame even though no guest stack arguments
  // exist; only calls with a real Android stack tail need extra scratch.
  plan->android_stack_size =
      guest_stack_size + (android_offset == 0 ? 0 : 16) + kJniThunkScratchBytes;
  return true;
}

bool PlanLifecycleShorty(const char* shorty,
                         ShortyPlan* plan,
                         std::string* error) {
  if (shorty == nullptr ||
      (std::strcmp(shorty, "ILL") != 0 && std::strcmp(shorty, "VLL") != 0)) {
    *error = "lifecycle shorty must be ILL or VLL";
    return false;
  }
  return PlanShorty(shorty, CallShape::kLifecycle, plan, error);
}

constexpr uint32_t EncodeUnsignedLoadStore(uint32_t opcode,
                                           uint32_t target_register,
                                           uint32_t base_register,
                                           size_t byte_offset,
                                           size_t scale) {
  return opcode | (static_cast<uint32_t>(byte_offset / scale) << 10) |
         (base_register << 5) | target_register;
}

uint32_t EncodeLoad(uint32_t target_register,
                    uint32_t base_register,
                    size_t byte_offset,
                    size_t size) {
  switch (size) {
    case 1:
      return EncodeUnsignedLoadStore(0x39400000u, target_register, base_register,
                                     byte_offset, 1);
    case 2:
      return EncodeUnsignedLoadStore(0x79400000u, target_register, base_register,
                                     byte_offset, 2);
    case 4:
      return EncodeUnsignedLoadStore(0xb9400000u, target_register, base_register,
                                     byte_offset, 4);
    default:
      return EncodeUnsignedLoadStore(0xf9400000u, target_register, base_register,
                                     byte_offset, 8);
  }
}

uint32_t EncodeStore(uint32_t target_register,
                     uint32_t base_register,
                     size_t byte_offset,
                     size_t size) {
  switch (size) {
    case 1:
      return EncodeUnsignedLoadStore(0x39000000u, target_register, base_register,
                                     byte_offset, 1);
    case 2:
      return EncodeUnsignedLoadStore(0x79000000u, target_register, base_register,
                                     byte_offset, 2);
    case 4:
      return EncodeUnsignedLoadStore(0xb9000000u, target_register, base_register,
                                     byte_offset, 4);
    default:
      return EncodeUnsignedLoadStore(0xf9000000u, target_register, base_register,
                                     byte_offset, 8);
  }
}

uint32_t EncodeVectorStore(uint32_t vector_register, uint32_t base_register,
                           size_t byte_offset) {
  return 0x3d800000u |
         (static_cast<uint32_t>(byte_offset / 16u) << 10) |
         (base_register << 5) | vector_register;
}

uint32_t EncodeVectorLoad(uint32_t vector_register, uint32_t base_register,
                          size_t byte_offset) {
  return 0x3dc00000u |
         (static_cast<uint32_t>(byte_offset / 16u) << 10) |
         (base_register << 5) | vector_register;
}

uint32_t EncodeSubSp(size_t byte_count) {
  return 0xd10003ffu | (static_cast<uint32_t>(byte_count) << 10);
}

uint32_t EncodeLdrLiteralX(uint32_t target_register,
                           size_t instruction_offset,
                           size_t literal_offset) {
  const size_t delta = literal_offset - instruction_offset;
  return 0x58000000u | (static_cast<uint32_t>(delta / 4u) << 5) |
         target_register;
}

void Write32(uint8_t* destination, size_t offset, uint32_t value) {
  std::memcpy(destination + offset, &value, sizeof(value));
}

void Write64(uint8_t* destination, size_t offset, uintptr_t value) {
  static_assert(sizeof(value) == sizeof(uint64_t));
  std::memcpy(destination + offset, &value, sizeof(value));
}

bool Publish(const RegistryEntry& entry) {
  std::lock_guard<std::mutex> lock(g_registry_mutex);
  for (RegistryEntry& slot : g_registry) {
    if (slot.generation == 0) {
      slot = entry;
      return true;
    }
  }
  g_registry.push_back(entry);
  return true;
}

void Unpublish(const RegistryEntry& entry) {
  std::lock_guard<std::mutex> lock(g_registry_mutex);
  for (RegistryEntry& slot : g_registry) {
    if (slot.generation == entry.generation && slot.start == entry.start) {
      slot = {};
      return;
    }
  }
}

std::string ErrnoMessage(const char* operation) {
  return std::string(operation) + ": " + std::strerror(errno);
}

struct GeneratedThunk {
  size_t offset;
  size_t size;
  uint32_t mask;
  size_t source_request;
};

struct CacheKey {
  uintptr_t target;
  std::string shorty;
  CallShape shape;

  bool operator==(const CacheKey& other) const {
    return target == other.target && shorty == other.shorty &&
           shape == other.shape;
  }
};

struct CacheKeyHash {
  size_t operator()(const CacheKey& key) const {
    return std::hash<uintptr_t>{}(key.target) ^
           (std::hash<std::string>{}(key.shorty) << 1u) ^
           (static_cast<size_t>(key.shape) << 2u);
  }
};

}  // namespace

struct TrampolineSet {
  void* mapping = nullptr;
  size_t mapping_size = 0;
  RegistryEntry registry;
  std::vector<uintptr_t> requested_entries;
};

static TrampolineSet* CreateTrampolines(void* proxy,
                                 const TrampolineRequest* requests,
                                 size_t request_count,
                                 std::string* error,
                                 CallShape shape) {
  if (error != nullptr) {
    error->clear();
  }
  std::string local_error;
  const bool needs_proxy = shape != CallShape::kCritical;
  if ((needs_proxy && proxy == nullptr) || requests == nullptr ||
      request_count == 0 || request_count > kMaxRequests) {
    local_error = shape == CallShape::kLifecycle
                      ? "invalid lifecycle trampoline request set"
                      : shape == CallShape::kCritical
                            ? "invalid CriticalNative trampoline request set"
                            : "invalid regular JNI trampoline request set";
    if (error) *error = local_error;
    return nullptr;
  }
  std::vector<ShortyPlan> plans(request_count);
  for (size_t index = 0; local_error.empty() && index < request_count; ++index) {
    const bool valid_shorty =
        shape == CallShape::kLifecycle
            ? PlanLifecycleShorty(requests[index].shorty, &plans[index],
                                  &local_error)
            : PlanShorty(requests[index].shorty, shape, &plans[index],
                         &local_error);
    if (requests[index].android_target == nullptr ||
        requests[index].entry_mask == 0 ||
        (requests[index].entry_mask & (requests[index].entry_mask - 1u)) != 0 ||
        !valid_shorty) {
      if (local_error.empty()) {
        local_error = shape == CallShape::kLifecycle
                          ? "invalid lifecycle target or entry identity"
                          : shape == CallShape::kCritical
                                ? "invalid CriticalNative target or entry identity"
                                : "invalid regular JNI target or entry identity";
      }
    }
    if (local_error.empty() && plans[index].android_stack_size > 4080) {
      local_error = shape == CallShape::kLifecycle
                        ? "lifecycle Android stack tail exceeds encoder limit"
                        : shape == CallShape::kCritical
                              ? "CriticalNative Android stack tail exceeds encoder limit"
                              : "regular JNI Android stack tail exceeds encoder limit";
    }
  }
  if (!local_error.empty()) {
    if (error != nullptr) {
      *error = local_error;
    }
    return nullptr;
  }

  std::vector<GeneratedThunk> generated;
  std::vector<size_t> request_to_generated(request_count);
  std::unordered_map<CacheKey, size_t, CacheKeyHash> cache;
  size_t generated_size = 0;
  for (size_t index = 0; index < request_count; ++index) {
    CacheKey key{reinterpret_cast<uintptr_t>(requests[index].android_target),
                 requests[index].shorty, shape};
    auto [position, inserted] = cache.emplace(std::move(key), generated.size());
    if (!inserted) {
      request_to_generated[index] = position->second;
      generated[position->second].mask |= requests[index].entry_mask;
      continue;
    }
    const bool substitutes_proxy = shape != CallShape::kCritical;
    const size_t instruction_count =
        (substitutes_proxy ? 51u : 50u) + plans[index].moves.size() * 2u;
    const size_t literal_count = substitutes_proxy ? 4u : 3u;
    const size_t thunk_size = instruction_count * kInstructionSize +
                              literal_count * kLiteralSize;
    generated_size = RoundUp(generated_size, 16);
    request_to_generated[index] = generated.size();
    generated.push_back(
        {generated_size, thunk_size, requests[index].entry_mask, index});
    generated_size += thunk_size;
  }

  const long page_size_result = sysconf(_SC_PAGESIZE);
  if (page_size_result <= 0 || generated_size == 0 ||
      generated_size > kMaxExecutableBytes) {
    if (error != nullptr) {
      *error = "generated regular JNI thunks exceed executable mapping limit";
    }
    return nullptr;
  }
  const size_t page_size = static_cast<size_t>(page_size_result);
  const size_t mapping_size = RoundUp(generated_size, page_size);
  if (mapping_size < generated_size || mapping_size > kMaxExecutableBytes) {
    if (error != nullptr) {
      *error = "round regular JNI trampoline mapping size";
    }
    return nullptr;
  }
  void* mapping = mmap(nullptr, mapping_size, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANON, -1, 0);
  if (mapping == MAP_FAILED) {
    if (error != nullptr) {
      *error = ErrnoMessage("mmap regular JNI trampoline page");
    }
    return nullptr;
  }
  auto* bytes = static_cast<uint8_t*>(mapping);
  constexpr uint32_t kX9 = 9;
  constexpr uint32_t kX29 = 29;
  constexpr uint32_t kSp = 31;
  const bool substitutes_proxy = shape != CallShape::kCritical;
  for (size_t index = 0; index < generated.size(); ++index) {
    const GeneratedThunk& thunk = generated[index];
    const size_t source_request = thunk.source_request;
    const ShortyPlan& plan = plans[source_request];
    const size_t scratch_offset = plan.android_stack_size - kJniThunkScratchBytes;
    size_t cursor = thunk.offset;
    Write32(bytes, cursor, 0xa9bf7bfdu);  // stp x29, x30, [sp, #-16]!
    cursor += 4;
    Write32(bytes, cursor, 0x910003fdu);  // mov x29, sp
    cursor += 4;
    Write32(bytes, cursor, EncodeSubSp(plan.android_stack_size));
    cursor += 4;
    for (const StackMove& move : plan.moves) {
      Write32(bytes, cursor,
              EncodeLoad(kX9, kX29,
                         kDarwinSavedFrameSize + move.darwin_offset,
                         move.size));
      cursor += 4;
      Write32(bytes, cursor,
              EncodeStore(kX9, kSp, move.android_offset, move.size));
      cursor += 4;
    }
    const size_t literal_count = substitutes_proxy ? 4u : 3u;
    const size_t push_literal =
        thunk.offset + thunk.size - literal_count * kLiteralSize;
    const size_t pop_literal = push_literal + kLiteralSize;
    const size_t proxy_literal = pop_literal + kLiteralSize;
    const size_t target_literal = thunk.offset + thunk.size - 8u;
    // NativeBridge thunks are the actual JNI entrypoint on Darwin and do not
    // pass through ART's quick JNI entrypoints. Publish the same managed frame
    // contract around the guest call so a concurrent remote unwind can cross
    // this host-only ABI boundary.
    for (uint32_t reg = 0; reg < 8; ++reg) {
      Write32(bytes, cursor,
              EncodeVectorStore(reg, kSp, scratch_offset + reg * 16u));
      cursor += 4;
    }
    for (uint32_t reg = 0; reg < 8; ++reg) {
      Write32(bytes, cursor,
              EncodeStore(reg, kSp, scratch_offset + 128u + reg * 8u, 8));
      cursor += 4;
    }
    Write32(bytes, cursor, 0xaa1c03e0u);  // mov x0, x28
    cursor += 4;
    Write32(bytes, cursor, EncodeLdrLiteralX(16, cursor, push_literal));
    cursor += 4;
    Write32(bytes, cursor, 0xd63f0200u);  // blr x16
    cursor += 4;
    for (uint32_t reg = 0; reg < 8; ++reg) {
      Write32(bytes, cursor,
              EncodeVectorLoad(reg, kSp, scratch_offset + reg * 16u));
      cursor += 4;
    }
    for (uint32_t reg = 0; reg < 8; ++reg) {
      Write32(bytes, cursor,
              EncodeLoad(reg, kSp, scratch_offset + 128u + reg * 8u, 8));
      cursor += 4;
    }
    if (substitutes_proxy) {
      Write32(bytes, cursor, EncodeLdrLiteralX(0, cursor, proxy_literal));
      cursor += 4;
    }
    Write32(bytes, cursor, EncodeLdrLiteralX(16, cursor, target_literal));
    cursor += 4;
    Write32(bytes, cursor, 0xd63f0200u);  // blr x16
    cursor += 4;
    Write32(bytes, cursor,
            EncodeStore(0, kSp, scratch_offset + 8u, 8));  // str x0, scratch+8
    cursor += 4;
    Write32(bytes, cursor,
            EncodeVectorStore(0, kSp, scratch_offset + 208u));  // str q0, result
    cursor += 4;
    Write32(bytes, cursor, EncodeLdrLiteralX(16, cursor, pop_literal));
    cursor += 4;
    Write32(bytes, cursor, 0xd63f0200u);  // blr x16
    cursor += 4;
    Write32(bytes, cursor,
            EncodeLoad(0, kSp, scratch_offset + 8u, 8));  // ldr x0, scratch+8
    cursor += 4;
    Write32(bytes, cursor,
            EncodeVectorLoad(0, kSp, scratch_offset + 208u));  // ldr q0, result
    cursor += 4;
    Write32(bytes, cursor, 0x910003bfu);  // mov sp, x29
    cursor += 4;
    Write32(bytes, cursor, 0xa8c17bfdu);  // ldp x29, x30, [sp], #16
    cursor += 4;
    Write32(bytes, cursor, 0xd65f03c0u);  // ret
    cursor += 4;
    Write32(bytes, cursor, 0xd503201fu);  // nop / literal alignment
    cursor += 4;
    if (cursor != push_literal) {
      if (error != nullptr) {
        *error = "internal regular JNI thunk layout mismatch";
      }
      munmap(mapping, mapping_size);
      return nullptr;
    }
    Write64(bytes, push_literal,
            reinterpret_cast<uintptr_t>(&darwin_art_unwindstack_push_quick_frame));
    Write64(bytes, pop_literal,
            reinterpret_cast<uintptr_t>(&darwin_art_unwindstack_pop_quick_frame));
    if (substitutes_proxy) {
      Write64(bytes, proxy_literal, reinterpret_cast<uintptr_t>(proxy));
    }
    Write64(bytes, target_literal,
            reinterpret_cast<uintptr_t>(
                requests[source_request].android_target));
  }

  __builtin___clear_cache(reinterpret_cast<char*>(mapping),
                          reinterpret_cast<char*>(mapping) + generated_size);
  if (mprotect(mapping, mapping_size, PROT_READ | PROT_EXEC) != 0) {
    if (error != nullptr) {
      *error = ErrnoMessage("mprotect regular JNI trampoline page RX");
    }
    munmap(mapping, mapping_size);
    return nullptr;
  }

  auto* trampolines = new (std::nothrow) TrampolineSet;
  if (trampolines == nullptr) {
    if (error != nullptr) {
      *error = "allocate regular JNI trampoline owner";
    }
    munmap(mapping, mapping_size);
    return nullptr;
  }
  const uintptr_t start = reinterpret_cast<uintptr_t>(mapping);
  trampolines->mapping = mapping;
  trampolines->mapping_size = mapping_size;
  trampolines->requested_entries.reserve(request_count);
  trampolines->registry.start = start;
  trampolines->registry.end = start + generated_size;
  trampolines->registry.generation =
      g_next_generation.fetch_add(1, std::memory_order_relaxed);
  for (size_t index = 0; index < generated.size(); ++index) {
    trampolines->registry.entries.push_back(
        {start + generated[index].offset, generated[index].mask});
  }
  for (size_t index = 0; index < request_count; ++index) {
    trampolines->requested_entries.push_back(
        start + generated[request_to_generated[index]].offset);
  }
  if (trampolines->registry.generation == 0 ||
      !Publish(trampolines->registry)) {
    if (error != nullptr) {
      *error = "publish regular JNI trampoline range";
    }
    munmap(mapping, mapping_size);
    delete trampolines;
    return nullptr;
  }
  g_live_count.fetch_add(1, std::memory_order_relaxed);
  return trampolines;
}

TrampolineSet* CreateRegularTrampolines(void* proxy_jni_env,
                                        const TrampolineRequest* requests,
                                        size_t request_count,
                                        std::string* error) {
  return CreateTrampolines(proxy_jni_env, requests, request_count, error,
                           CallShape::kRegular);
}

TrampolineSet* CreateCriticalTrampolines(const TrampolineRequest* requests,
                                         size_t request_count,
                                         std::string* error) {
  return CreateTrampolines(nullptr, requests, request_count, error,
                           CallShape::kCritical);
}

TrampolineSet* CreateLifecycleTrampolines(void* proxy_java_vm,
                                          const TrampolineRequest* requests,
                                          size_t request_count,
                                          std::string* error) {
  return CreateTrampolines(proxy_java_vm, requests, request_count, error,
                           CallShape::kLifecycle);
}

void DestroyRegularTrampolines(TrampolineSet* trampolines) {
  if (trampolines == nullptr) {
    return;
  }
  Unpublish(trampolines->registry);
  g_live_count.fetch_sub(1, std::memory_order_relaxed);
  if (trampolines->mapping != nullptr && trampolines->mapping_size != 0) {
    munmap(trampolines->mapping, trampolines->mapping_size);
  }
  delete trampolines;
}

size_t TrampolineCount(const TrampolineSet* trampolines) {
  return trampolines == nullptr ? 0 : trampolines->requested_entries.size();
}

void* TrampolineEntry(const TrampolineSet* trampolines, size_t index) {
  return trampolines == nullptr || index >= trampolines->requested_entries.size()
             ? nullptr
             : reinterpret_cast<void*>(trampolines->requested_entries[index]);
}

uint64_t TrampolineGeneration(const TrampolineSet* trampolines) {
  return trampolines == nullptr ? 0 : trampolines->registry.generation;
}

size_t TrampolineLiveCount() {
  return g_live_count.load(std::memory_order_relaxed);
}

uint32_t TrampolineEntryMask(const void* pointer) {
  const uintptr_t address = reinterpret_cast<uintptr_t>(pointer);
  std::lock_guard<std::mutex> lock(g_registry_mutex);
  for (const RegistryEntry& registry : g_registry) {
    if (registry.generation == 0 || address < registry.start ||
        address >= registry.end) {
      continue;
    }
    for (const PublishedEntry& entry : registry.entries) {
      if (entry.address == address) {
        return entry.mask;
      }
    }
  }
  return 0;
}

bool IsTrampolineEntry(const void* pointer) {
  return TrampolineEntryMask(pointer) != 0;
}

}  // namespace darwin_art::android_jni
