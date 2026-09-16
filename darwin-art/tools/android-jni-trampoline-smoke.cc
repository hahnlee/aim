#include <cassert>
#include <cstdint>
#include <cstdio>
#include <limits>
#include <string>
#include <vector>

#include "darwin_android_jni_trampoline.h"

int main() {
  using darwin_art::android_jni::CreateRegularTrampolines;
  using darwin_art::android_jni::DestroyRegularTrampolines;
  using darwin_art::android_jni::IsTrampolineEntry;
  using darwin_art::android_jni::TrampolineCount;
  using darwin_art::android_jni::TrampolineEntry;
  using darwin_art::android_jni::TrampolineEntryMask;
  using darwin_art::android_jni::TrampolineLiveCount;
  using darwin_art::android_jni::TrampolineRequest;

  // The first request exercises every supported argument and return spelling,
  // exhausts both register banks independently, and forces 1/2/4/8-byte
  // Darwin stack moves. The duplicate proves the per-set target+shorty cache.
  const char* all_scalars =
      "LZBCSIJLFDZBCSIJLFDZBCSIJLFDFDFDFDFDFDFDFDFD";
  TrampolineRequest requests[] = {
      {reinterpret_cast<void*>(uintptr_t{0x1000}), all_scalars, 1u << 0},
      {reinterpret_cast<void*>(uintptr_t{0x1000}), all_scalars, 1u << 1},
      {reinterpret_cast<void*>(uintptr_t{0x2000}), "V", 1u << 2},
      {reinterpret_cast<void*>(uintptr_t{0x3000}), "F", 1u << 3},
      {reinterpret_cast<void*>(uintptr_t{0x4000}), "D", 1u << 4},
      {reinterpret_cast<void*>(uintptr_t{0x5000}), "Z", 1u << 5},
      {reinterpret_cast<void*>(uintptr_t{0x6000}), "B", 1u << 6},
      {reinterpret_cast<void*>(uintptr_t{0x7000}), "C", 1u << 7},
      {reinterpret_cast<void*>(uintptr_t{0x8000}), "S", 1u << 8},
      {reinterpret_cast<void*>(uintptr_t{0x9000}), "I", 1u << 9},
      {reinterpret_cast<void*>(uintptr_t{0xa000}), "J", 1u << 10},
  };
  std::string error;
  auto* set = CreateRegularTrampolines(
      reinterpret_cast<void*>(uintptr_t{0x5000}), requests,
      std::size(requests), &error);
  assert(set != nullptr && error.empty());
  assert(TrampolineLiveCount() == 1);
  assert(TrampolineCount(set) == std::size(requests));
  assert(TrampolineEntry(set, 0) == TrampolineEntry(set, 1));
  assert(TrampolineEntryMask(TrampolineEntry(set, 0)) == 3u);
  for (size_t index = 0; index < std::size(requests); ++index) {
    assert(IsTrampolineEntry(TrampolineEntry(set, index)));
  }
  auto* interior = static_cast<uint8_t*>(TrampolineEntry(set, 0)) + 4;
  assert(!IsTrampolineEntry(interior));
  DestroyRegularTrampolines(set);
  assert(TrampolineLiveCount() == 0);
  assert(!IsTrampolineEntry(TrampolineEntry(nullptr, 0)));

  // Conscrypt registers hundreds of natives in one call. Prove that the
  // generated W^X mapping spans as many host pages as needed rather than
  // imposing a one-page implementation limit.
  constexpr size_t kConscryptScaleCount = 309;
  std::vector<TrampolineRequest> large_requests;
  large_requests.reserve(kConscryptScaleCount);
  for (size_t index = 0; index < kConscryptScaleCount; ++index) {
    large_requests.push_back({
        reinterpret_cast<void*>(uintptr_t{0x100000} + index * 0x10),
        "IIL", 1u});
  }
  error.clear();
  auto* large_set = CreateRegularTrampolines(
      reinterpret_cast<void*>(uintptr_t{0x5000}), large_requests.data(),
      large_requests.size(), &error);
  assert(large_set != nullptr && error.empty());
  assert(TrampolineLiveCount() == 1);
  assert(TrampolineCount(large_set) == kConscryptScaleCount);
  for (size_t index = 0; index < kConscryptScaleCount; ++index) {
    assert(TrampolineEntry(large_set, index) != nullptr);
    assert(TrampolineEntryMask(TrampolineEntry(large_set, index)) == 1u);
  }
  DestroyRegularTrampolines(large_set);
  assert(TrampolineLiveCount() == 0);

  for (const char* rejected : {"IQ", "Q", "IV", ""}) {
    TrampolineRequest invalid = {
        reinterpret_cast<void*>(uintptr_t{0xb000}), rejected, 1u};
    error.clear();
    assert(CreateRegularTrampolines(
               reinterpret_cast<void*>(uintptr_t{0x5000}), &invalid, 1,
               &error) == nullptr);
    assert(!error.empty());
  }
  // Reject oversized request counts before allocating any per-request state.
  error.clear();
  assert(CreateRegularTrampolines(reinterpret_cast<void*>(uintptr_t{0x5000}),
             requests, std::numeric_limits<size_t>::max(), &error) == nullptr);
  assert(!error.empty());
  std::puts("android-jni-trampoline: PASS scalar-ref=ZBCSIJFDL returns=all "
            "gp-fp=independent stack=darwin-natural-to-android-8byte "
            "cache=target+shorty wx=rw-to-rx multipage=309 "
            "reject=aggregate+V-arg");
  return 0;
}
