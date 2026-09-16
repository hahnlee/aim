// ARM64 ABI smoke test for the three JNI trampoline entry shapes.
//
// The target functions below are naked AArch64 assembly on purpose.  A normal
// Darwin C++ callee is allowed to consume Darwin's naturally packed narrow
// stack arguments, so it cannot prove that the trampoline produced Android's
// eight-byte AAPCS64 stack slots or kept the GP and FP banks independent.

#include <cassert>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <iterator>
#include <string>

#include "darwin_android_jni_trampoline.h"

#if !defined(__aarch64__)
#error "android-jni-call-shape-smoke requires an Apple arm64 target"
#endif

using darwin_art::android_jni::CreateCriticalTrampolines;
using darwin_art::android_jni::CreateLifecycleTrampolines;
using darwin_art::android_jni::DestroyRegularTrampolines;
using darwin_art::android_jni::TrampolineEntry;
using darwin_art::android_jni::TrampolineLiveCount;
using darwin_art::android_jni::TrampolineRequest;

extern "C" {
volatile uintptr_t g_lifecycle_one_vm = 0;
volatile uintptr_t g_lifecycle_one_reserved = 0;
volatile uintptr_t g_lifecycle_two_vm = 0;
volatile uintptr_t g_lifecycle_two_reserved = 0;
}

using CriticalEntry = uint64_t (*)(uint8_t, uint8_t, uint16_t, uint16_t,
                                   uint32_t, uint64_t, uint32_t, uint64_t,
                                   uint8_t, uint16_t, uint32_t, float, float, float, float, float,
                                   float, float, float, double);
using LifecycleLoad = int32_t (*)(void *, void *);
using LifecycleUnload = void (*)(void *, void *);

// UBSan's indirect-call check expects compiler metadata immediately before a
// normal function entry. Generated RX entries intentionally have a different
// layout (their first literal is not metadata), so keep the sanitizer bypass
// limited to these three dynamic-call wrappers.
__attribute__((no_sanitize("function"))) uint64_t
InvokeCritical(CriticalEntry entry, uint8_t gp0, uint8_t gp1, uint16_t gp2,
               uint16_t gp3, uint32_t gp4, uint64_t gp5, uint32_t gp6,
               uint64_t gp7, uint8_t gp8, uint16_t gp9, uint32_t gp10, float f0, float f1, float f2,
               float f3, float f4, float f5, float f6, float f7, double d8) {
  return entry(gp0, gp1, gp2, gp3, gp4, gp5, gp6, gp7, gp8, gp9, gp10, f0, f1, f2, f3, f4,
               f5, f6, f7, d8);
}

__attribute__((no_sanitize("function"))) int32_t
InvokeLifecycleLoad(LifecycleLoad entry, void *reserved) {
  return entry(nullptr, reserved);
}

__attribute__((no_sanitize("function"))) void
InvokeLifecycleUnload(LifecycleUnload entry, void *reserved) {
  entry(nullptr, reserved);
}

extern "C" __attribute__((naked, noinline)) uint64_t
CriticalStackAndFpTarget(uint8_t, uint8_t, uint16_t, uint16_t, uint32_t,
                         uint64_t, uint32_t, uint64_t, uint8_t, uint16_t, uint32_t, float, float,
                         float, float, float, float, float, float, double) {
  asm volatile(
      // CriticalNative has no JNIEnv*/jobject prefix: GP arguments begin at
      // x0. Normalize narrow register arguments before combining them.
      "and x16, x0, #0xff\n"
      "and x9, x1, #0xff\n"
      "eor x16, x16, x9\n"
      "and x9, x2, #0xffff\n"
      "eor x16, x16, x9\n"
      "and x9, x3, #0xffff\n"
      "eor x16, x16, x9\n"
      "mov w9, w4\n"
      "eor x16, x16, x9\n"
      "eor x16, x16, x5\n"
      "mov w9, w6\n"
      "eor x16, x16, x9\n"
      "eor x16, x16, x7\n"
      // The ninth GP argument is a narrow value in Android's first eight-byte
      // stack slot. Two more narrow integers occupy offsets8/16, rather than
      // Darwin's packed offsets2/4. The spilled double occupies offset24.
      "ldr x9, [sp, #0]\n"
      "and x9, x9, #0xff\n"
      "eor x16, x16, x9\n"
      "ldrh w9, [sp, #8]\n"
      "eor x16, x16, x9\n"
      "ldr w9, [sp, #16]\n"
      "eor x16, x16, x9\n"
      // Eight independent FP registers precede the spilled double.
      "fmov w9, s0\n"
      "eor x16, x16, x9\n"
      "fmov w9, s1\n"
      "eor x16, x16, x9\n"
      "fmov w9, s2\n"
      "eor x16, x16, x9\n"
      "fmov w9, s3\n"
      "eor x16, x16, x9\n"
      "fmov w9, s4\n"
      "eor x16, x16, x9\n"
      "fmov w9, s5\n"
      "eor x16, x16, x9\n"
      "fmov w9, s6\n"
      "eor x16, x16, x9\n"
      "fmov w9, s7\n"
      "eor x16, x16, x9\n"
      "ldr x9, [sp, #24]\n"
      "eor x16, x16, x9\n"
      "mov x0, x16\n"
      "ret\n");
}

// Constants are deliberately non-pointer-like and distinct per owner. The
// target compares the received x0/x1 values, proving proxy substitution and
// reserved-argument passthrough without dereferencing either value.
extern "C" __attribute__((naked, noinline)) int32_t LifecycleLoadOne(void *,
                                                                     void *) {
  asm volatile("movz x9, #0x3344\n"
               "movk x9, #0x2222, lsl #16\n"
               "movk x9, #0x1111, lsl #32\n"
               "movk x9, #0x0000, lsl #48\n"
               "cmp x0, x9\n"
               "b.ne 1f\n"
               "movz x9, #0x7788\n"
               "movk x9, #0x5566, lsl #16\n"
               "movk x9, #0x3344, lsl #32\n"
               "movk x9, #0x1122, lsl #48\n"
               "cmp x1, x9\n"
               "b.ne 1f\n"
               "mov w0, #0x1234\n"
               "ret\n"
               "1:\n"
               "mov w0, #-1\n"
               "ret\n");
}

extern "C" __attribute__((naked, noinline)) int32_t LifecycleLoadTwo(void *,
                                                                     void *) {
  asm volatile("movz x9, #0xbbcc\n"
               "movk x9, #0x99aa, lsl #16\n"
               "movk x9, #0x7766, lsl #32\n"
               "movk x9, #0x5544, lsl #48\n"
               "cmp x0, x9\n"
               "b.ne 1f\n"
               "movz x9, #0x2211\n"
               "movk x9, #0x4433, lsl #16\n"
               "movk x9, #0x6655, lsl #32\n"
               "movk x9, #0x8877, lsl #48\n"
               "cmp x1, x9\n"
               "b.ne 1f\n"
               "mov w0, #0x5678\n"
               "ret\n"
               "1:\n"
               "mov w0, #-1\n"
               "ret\n");
}

extern "C" __attribute__((naked, noinline)) void LifecycleUnloadOne(void *,
                                                                    void *) {
  asm volatile("adrp x9, _g_lifecycle_one_vm@PAGE\n"
               "add x9, x9, _g_lifecycle_one_vm@PAGEOFF\n"
               "str x0, [x9]\n"
               "adrp x9, _g_lifecycle_one_reserved@PAGE\n"
               "add x9, x9, _g_lifecycle_one_reserved@PAGEOFF\n"
               "str x1, [x9]\n"
               "ret\n");
}

extern "C" __attribute__((naked, noinline)) void LifecycleUnloadTwo(void *,
                                                                    void *) {
  asm volatile("adrp x9, _g_lifecycle_two_vm@PAGE\n"
               "add x9, x9, _g_lifecycle_two_vm@PAGEOFF\n"
               "str x0, [x9]\n"
               "adrp x9, _g_lifecycle_two_reserved@PAGE\n"
               "add x9, x9, _g_lifecycle_two_reserved@PAGEOFF\n"
               "str x1, [x9]\n"
               "ret\n");
}

template <typename Function> void *Target(Function function) {
  return reinterpret_cast<void *>(reinterpret_cast<uintptr_t>(function));
}

uint64_t FloatBits(float value) {
  uint32_t bits = 0;
  std::memcpy(&bits, &value, sizeof(bits));
  return bits;
}

uint64_t DoubleBits(double value) {
  uint64_t bits = 0;
  std::memcpy(&bits, &value, sizeof(bits));
  return bits;
}

void RejectCritical(const char *shorty) {
  TrampolineRequest request{Target(&CriticalStackAndFpTarget), shorty, 1};
  std::string error;
  assert(CreateCriticalTrampolines(&request, 1, &error) == nullptr);
  assert(!error.empty());
}

void RejectLifecycle(const char *shorty) {
  TrampolineRequest request{Target(&LifecycleLoadOne), shorty, 1};
  std::string error;
  assert(CreateLifecycleTrampolines(reinterpret_cast<void *>(1), &request, 1,
                                    &error) == nullptr);
  assert(!error.empty());
}

int main() {
  assert(TrampolineLiveCount() == 0);

  // Eleven GP arguments (three narrow spills) and nine FP arguments
  // (the ninth is a double spill) prove no implicit regular-JNI prefix and
  // independent register-bank accounting. The shorty is return J followed by
  // GP ZBCSIJIJBCI and FP FFFFFFFFD.
  constexpr const char *kCriticalShorty = "JZBCSIJIJBCIFFFFFFFFD";
  constexpr uint8_t kGp0 = 0x11;
  constexpr uint8_t kGp1 = 0x22;
  constexpr uint16_t kGp2 = 0x3344;
  constexpr uint16_t kGp3 = 0x5566;
  constexpr uint32_t kGp4 = 0x778899aa;
  constexpr uint64_t kGp5 = 0x1122334455667788ull;
  constexpr uint32_t kGp6 = 0xaabbccdd;
  constexpr uint64_t kGp7 = 0x8877665544332211ull;
  constexpr uint8_t kGp8 = 0xee;
  constexpr uint16_t kGp9 = 0xabcd;
  constexpr uint32_t kGp10 = 0x12345678;
  constexpr float kF0 = 1.25f;
  constexpr float kF1 = 2.5f;
  constexpr float kF2 = 3.75f;
  constexpr float kF3 = 4.125f;
  constexpr float kF4 = 5.25f;
  constexpr float kF5 = 6.5f;
  constexpr float kF6 = 7.75f;
  constexpr float kF7 = 8.875f;
  constexpr double kD8 = 9.5;

  TrampolineRequest critical_request{Target(&CriticalStackAndFpTarget),
                                     kCriticalShorty, 1};
  std::string error;
  auto *critical = CreateCriticalTrampolines(&critical_request, 1, &error);
  assert(critical != nullptr && error.empty());
  assert(TrampolineLiveCount() == 1);
  auto critical_entry =
      reinterpret_cast<CriticalEntry>(TrampolineEntry(critical, 0));
  const uint64_t expected = kGp0 ^ kGp1 ^ kGp2 ^ kGp3 ^ kGp4 ^ kGp5 ^ kGp6 ^
                            kGp7 ^ kGp8 ^ kGp9 ^ kGp10 ^ FloatBits(kF0) ^ FloatBits(kF1) ^
                            FloatBits(kF2) ^ FloatBits(kF3) ^ FloatBits(kF4) ^
                            FloatBits(kF5) ^ FloatBits(kF6) ^ FloatBits(kF7) ^
                            DoubleBits(kD8);
  assert(InvokeCritical(critical_entry, kGp0, kGp1, kGp2, kGp3, kGp4, kGp5,
                        kGp6, kGp7, kGp8, kGp9, kGp10, kF0, kF1, kF2, kF3, kF4, kF5, kF6,
                        kF7, kD8) == expected);

  RejectCritical("L");
  RejectCritical("JLI");
  assert(TrampolineLiveCount() == 1);

  constexpr uintptr_t kProxyOne = 0x0000111122223344ull;
  constexpr uintptr_t kReservedOne = 0x1122334455667788ull;
  constexpr uintptr_t kProxyTwo = 0x5544776699aabbccull;
  constexpr uintptr_t kReservedTwo = 0x8877665544332211ull;
  TrampolineRequest owner_one_requests[] = {
      {Target(&LifecycleLoadOne), "ILL", 2},
      {Target(&LifecycleUnloadOne), "VLL", 4},
  };
  TrampolineRequest owner_two_requests[] = {
      {Target(&LifecycleLoadTwo), "ILL", 8},
      {Target(&LifecycleUnloadTwo), "VLL", 16},
  };
  auto *owner_one = CreateLifecycleTrampolines(
      reinterpret_cast<void *>(kProxyOne), owner_one_requests,
      std::size(owner_one_requests), &error);
  assert(owner_one != nullptr && error.empty());
  auto *owner_two = CreateLifecycleTrampolines(
      reinterpret_cast<void *>(kProxyTwo), owner_two_requests,
      std::size(owner_two_requests), &error);
  assert(owner_two != nullptr && error.empty());
  assert(TrampolineLiveCount() == 3);

  auto load_one =
      reinterpret_cast<LifecycleLoad>(TrampolineEntry(owner_one, 0));
  auto unload_one =
      reinterpret_cast<LifecycleUnload>(TrampolineEntry(owner_one, 1));
  auto load_two =
      reinterpret_cast<LifecycleLoad>(TrampolineEntry(owner_two, 0));
  auto unload_two =
      reinterpret_cast<LifecycleUnload>(TrampolineEntry(owner_two, 1));

  // Pass a deliberately wrong first argument: lifecycle thunks must substitute
  // each owner's configured proxy JavaVM. Keep both owners live and interleave
  // their calls so correctness cannot depend on TLS or a single global owner.
  assert(InvokeLifecycleLoad(load_one,
                             reinterpret_cast<void *>(kReservedOne)) == 0x1234);
  assert(InvokeLifecycleLoad(load_two,
                             reinterpret_cast<void *>(kReservedTwo)) == 0x5678);
  InvokeLifecycleUnload(unload_two, reinterpret_cast<void *>(kReservedTwo));
  InvokeLifecycleUnload(unload_one, reinterpret_cast<void *>(kReservedOne));
  assert(InvokeLifecycleLoad(load_two,
                             reinterpret_cast<void *>(kReservedTwo)) == 0x5678);
  assert(InvokeLifecycleLoad(load_one,
                             reinterpret_cast<void *>(kReservedOne)) == 0x1234);
  InvokeLifecycleUnload(unload_one, reinterpret_cast<void *>(kReservedOne));
  InvokeLifecycleUnload(unload_two, reinterpret_cast<void *>(kReservedTwo));
  assert(g_lifecycle_one_vm == kProxyOne);
  assert(g_lifecycle_one_reserved == kReservedOne);
  assert(g_lifecycle_two_vm == kProxyTwo);
  assert(g_lifecycle_two_reserved == kReservedTwo);

  for (const char *shorty : {"ILLX", "VLLI", "V", "I"}) {
    RejectLifecycle(shorty);
  }
  assert(TrampolineLiveCount() == 3);

  DestroyRegularTrampolines(owner_two);
  DestroyRegularTrampolines(owner_one);
  DestroyRegularTrampolines(critical);
  assert(TrampolineLiveCount() == 0);

  std::puts("android-jni-call-shape: PASS critical=no-prefix gp-spill=narrow "
            "fp-bank=independent fp-spill=8byte lifecycle=ILL+VLL "
            "proxy=substituted reserved=passthrough owners=interleaved "
            "tls=none reject=refs+invalid registry-cleanup=0");
  return 0;
}
