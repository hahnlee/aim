// A regular JNI thunk publishes x28 as the managed SP around the guest call.
// Under a compiled JNI stub x28 holds an arbitrary callee-saved value, so the
// thunk must neither fault nor leave the quick-frame registry unbalanced, and
// only an on-stack, 16-byte aligned SP may be recorded as a managed frame.
#include <pthread.h>

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <string>

#include "darwin_android_jni_trampoline.h"
#include "darwin_unwindstack_native.h"

namespace {

using darwin_art::android_jni::CreateRegularTrampolines;
using darwin_art::android_jni::DestroyRegularTrampolines;
using darwin_art::android_jni::TrampolineEntry;
using darwin_art::android_jni::TrampolineRequest;

int failures = 0;

void Check(bool condition, const char* what) {
  if (!condition) {
    std::fprintf(stderr, "FAIL %s\n", what);
    ++failures;
  }
}

uint64_t ThreadId() {
  uint64_t id = 0;
  pthread_threadid_np(nullptr, &id);
  return id;
}

// This thread's registry depth and innermost key (0 when it holds no slot).
uint64_t Depth(uint64_t* top_key = nullptr) {
  const uint64_t self = ThreadId();
  for (const DarwinArtQuickFrameSlot& slot : darwin_art_unwindstack_quick_frames.slots) {
    if (__atomic_load_n(&slot.thread_id, __ATOMIC_ACQUIRE) != self) continue;
    const uint64_t depth = __atomic_load_n(&slot.depth, __ATOMIC_ACQUIRE);
    if (top_key != nullptr && depth != 0) *top_key = slot.frame_keys[depth - 1];
    return depth;
  }
  return 0;
}

void* const kProxyEnv = reinterpret_cast<void*>(0x5eed);
uint64_t depth_in_call = 0;
uint64_t key_in_call = 0;
void* env_in_call = nullptr;

// The guest (Android ABI) JNI target: jint f(JNIEnv*, jclass, jint).
int32_t Target(void* env, void*, int32_t value) {
  env_in_call = env;
  key_in_call = 0;
  depth_in_call = Depth(&key_in_call);
  return value + 1;
}

}  // namespace

// Calls `entry(env, clazz, value)` with x28 set to `x28`, restoring the
// caller's x28 (callee-saved) afterwards, as a compiled JNI stub would leave it.
extern "C" int32_t CallWithX28(void* entry, uint64_t x28, void* env, void* clazz,
                               int32_t value);
asm(R"(
  .text
  .p2align 2
  .globl _CallWithX28
_CallWithX28:
  stp x29, x30, [sp, #-32]!
  mov x29, sp
  str x28, [sp, #16]
  mov x16, x0
  mov x28, x1
  mov x0, x2
  mov x1, x3
  mov w2, w4
  blr x16
  ldr x28, [sp, #16]
  ldp x29, x30, [sp], #32
  ret
)");

namespace {

struct Case {
  const char* name;
  uint64_t x28;
  bool managed;
};

void Run(void* entry, const Case& c) {
  depth_in_call = 99;
  const int32_t result = CallWithX28(entry, c.x28, nullptr, nullptr, 41);
  std::string what = std::string(c.name) + ": ";
  Check(result == 42, (what + "guest result").c_str());
  Check(env_in_call == kProxyEnv, (what + "proxy JNIEnv substituted").c_str());
  Check(depth_in_call == (c.managed ? 1u : 0u), (what + "frame published only for a managed SP").c_str());
  if (c.managed) Check(key_in_call == c.x28, (what + "frame keyed by x28").c_str());
  Check(Depth() == 0, (what + "registry balanced after the call").c_str());
}

}  // namespace

int main() {
  const TrampolineRequest request{reinterpret_cast<void*>(&Target), "II", 1};
  std::string error;
  auto* set = CreateRegularTrampolines(kProxyEnv, &request, 1, &error);
  if (set == nullptr) {
    std::fprintf(stderr, "FAIL create trampoline: %s\n", error.c_str());
    return 1;
  }
  void* entry = TrampolineEntry(set, 0);

  // A plausible managed SP: 16-byte aligned on this stack with the 28 frame
  // words the push copies above it.
  alignas(16) uint64_t frame[40] = {};
  const uint64_t on_stack = reinterpret_cast<uint64_t>(&frame[0]);
  const Case cases[] = {
      {"null", 0, false},
      {"garbage", 0xdeadbeefcafef00dull, false},
      {"unmapped", 0x10, false},
      {"heap", reinterpret_cast<uint64_t>(std::malloc(256)), false},
      {"misaligned stack", on_stack + 8, false},
      {"managed sp", on_stack, true},
      // A stale value after a managed call must not pop anything.
      {"garbage again", 0x0000700000000000ull, false},
  };
  for (const Case& c : cases) Run(entry, c);

  DestroyRegularTrampolines(set);
  if (failures != 0) return 1;
  std::printf("jni-thunk managed-sp: %zu x28 values PASS\n", std::size(cases));
  return 0;
}
