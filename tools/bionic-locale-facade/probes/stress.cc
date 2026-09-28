#include "aim_bionic_locale.h"

#include <atomic>
#include <cstdint>
#include <cstdio>
#include <thread>
#include <vector>

extern "C" int32_t aim_bionic_errno_load(void);
extern "C" void aim_bionic_errno_store(int32_t value);

namespace {

constexpr int kThreads = 8;
constexpr int kRounds = 100;
constexpr int kAndroidEilseq = 84;

int RunWorker(int index) {
  const bool utf8 = (index & 1) != 0;
  for (int round = 0; round < kRounds; ++round) {
    AimAndroidLocale locale = aim_bionic_newlocale(
        0x1fbf, utf8 ? "C.UTF-8" : "C", nullptr);
    if (locale == nullptr ||
        aim_bionic_uselocale(locale) !=
            reinterpret_cast<AimAndroidLocale>(UINTPTR_MAX) ||
        aim_bionic___ctype_get_mb_cur_max() != (utf8 ? 4U : 1U)) {
      return 1;
    }

    AimAndroidMbState state{};
    uint32_t output = 0;
    if (aim_bionic_mbrtowc(&output, "\xf0\x9f", 2, &state) !=
            static_cast<size_t>(-2) ||
        aim_bionic_mbrtowc(&output, "\x98\x80", 2, &state) != 2 ||
        output != 0x1f600) {
      return 2;
    }
    aim_bionic_errno_store(0);
    if (aim_bionic_mbrtowc(&output, "\xed\xa0\x80", 3, &state) !=
            static_cast<size_t>(-1) ||
        aim_bionic_errno_load() != kAndroidEilseq ||
        aim_bionic_mbrtowc(&output, "A", 1, &state) != 1) {
      return 3;
    }

    char encoded[4]{};
    if (aim_bionic_wcrtomb(encoded, 0x10ffff, &state) != 4 ||
        aim_bionic_wcrtomb(encoded, 0xd800, &state) != 3) {
      return 4;
    }
    for (int iteration = 0; iteration < 100; ++iteration) {
      if (aim_bionic_iswalpha_l(0x03b1, locale) == 0 ||
          aim_bionic_iswlower_l(0x03b1, locale) == 0 ||
          aim_bionic_iswupper_l(0x03b1, locale) != 0 ||
          aim_bionic_towupper_l(0x03b1, locale) != 0x0391 ||
          aim_bionic_iswprint_l(0x1f600, locale) == 0 ||
          aim_bionic_iswalpha_l(UINT32_MAX, locale) != 0 ||
          aim_bionic_towlower_l(UINT32_MAX, locale) != UINT32_MAX) {
        return 6;
      }
    }
    if (aim_bionic_uselocale(
            reinterpret_cast<AimAndroidLocale>(UINTPTR_MAX)) != locale) {
      return 5;
    }
    aim_bionic_freelocale(locale);
  }
  return 0;
}

}  // namespace

int main() {
  aim_bionic_locale_test_prepare_host_state();
  std::atomic<int> failure{};
  std::vector<std::thread> workers;
  for (int index = 0; index < kThreads; ++index) {
    workers.emplace_back([&, index] {
      const int result = RunWorker(index);
      if (result != 0) failure.store(result, std::memory_order_relaxed);
    });
  }
  for (std::thread& worker : workers) worker.join();
  if (failure.load(std::memory_order_relaxed) != 0 ||
      aim_bionic_locale_live_handle_count() != 0 ||
      aim_bionic_locale_test_host_state_is_preserved() == 0) {
    return 1;
  }
  std::puts("bionic-locale-stress: PASS threads=8 rounds=100 handles=0 UTF-8-invalid+incomplete ICU76-wide=concurrent state=thread-local ASan+UBSan=clean");
  return 0;
}
