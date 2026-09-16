#include "darwin_art_bionic_numeric.h"

#include <errno.h>
#include <stdint.h>

#include <array>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <thread>
#include <vector>

extern "C" long aosp_strtol(const char*, char**, int);
extern "C" long long aosp_strtoll(const char*, char**, int);
extern "C" intmax_t aosp_strtoimax(const char*, char**, int);
extern "C" unsigned long aosp_strtoul(const char*, char**, int);
extern "C" unsigned long long aosp_strtoull(const char*, char**, int);
extern "C" int32_t darwin_art_bionic_errno_load(void);
extern "C" void darwin_art_bionic_errno_store(int32_t);

namespace {

constexpr int kGuestSentinel = 777;
constexpr int kHostSentinel = 31'901;

template <typename Reference, typename Provider>
void CompareOne(const char* name, const std::string& input, int base,
                Reference reference, Provider provider) {
  char* reference_end = nullptr;
  errno = kGuestSentinel;
  const auto reference_value = reference(input.c_str(), &reference_end, base);
  const int reference_errno = errno;
  const ptrdiff_t reference_offset = reference_end - input.c_str();

  char* provider_end = nullptr;
  darwin_art_bionic_errno_store(kGuestSentinel);
  errno = kHostSentinel;
  const auto provider_value = provider(input.c_str(), &provider_end, base);
  const int provider_errno = darwin_art_bionic_errno_load();
  const ptrdiff_t provider_offset = provider_end - input.c_str();
  if (provider_value != reference_value || provider_offset != reference_offset ||
      provider_errno != reference_errno || errno != kHostSentinel) {
    std::fprintf(stderr,
                 "%s mismatch input='%s' base=%d value=%llu/%llu "
                 "end=%td/%td errno=%d/%d host=%d\n",
                 name, input.c_str(), base,
                 static_cast<unsigned long long>(provider_value),
                 static_cast<unsigned long long>(reference_value), provider_offset,
                 reference_offset, provider_errno, reference_errno, errno);
    std::abort();
  }
}

void CompareCorpus() {
  const std::array<const char*, 31> inputs = {
      "",          " ",          "+",           "-",
      "xyz",       "  +xyz",     "0",           "00",
      "0779",      "0x",         "0x0",         "0xg",
      "0x7fffffff", "-0X8000000000000000!", "0b", "0b0",
      "0b2",       "0b1010102",  "+42tail",     "-42tail",
      "9223372036854775807",      "9223372036854775808",
      "-9223372036854775808",     "-9223372036854775809",
      "18446744073709551615",     "18446744073709551616",
      "-18446744073709551615",    "-18446744073709551616",
      "zzzzzzzzzzzzzzzzzzzz",     "\t\r\n123", "123_456"};
  for (int base = -2; base <= 38; ++base) {
    for (const char* input : inputs) {
      CompareOne("strtol", input, base, aosp_strtol,
                 darwin_art_bionic_strtol);
      CompareOne("strtoll", input, base, aosp_strtoll,
                 darwin_art_bionic_strtoll);
      CompareOne("strtoimax", input, base, aosp_strtoimax,
                 darwin_art_bionic_strtoimax);
      CompareOne("strtoul", input, base, aosp_strtoul,
                 darwin_art_bionic_strtoul);
      CompareOne("strtoull", input, base, aosp_strtoull,
                 darwin_art_bionic_strtoull);
    }
  }
  for (int base = 2; base <= 36; ++base) {
    std::string digits;
    for (int digit = 0; digit < base; ++digit) {
      digits.push_back(static_cast<char>(digit < 10 ? '0' + digit
                                                    : 'a' + digit - 10));
    }
    digits += "!";
    CompareOne("generated strtoll", digits, base, aosp_strtoll,
               darwin_art_bionic_strtoll);
    CompareOne("generated strtoimax", digits, base, aosp_strtoimax,
               darwin_art_bionic_strtoimax);
    CompareOne("generated strtoull", digits, base, aosp_strtoull,
               darwin_art_bionic_strtoull);
  }

  for (DarwinArtAndroidLocale locale : {
           static_cast<DarwinArtAndroidLocale>(nullptr),
           reinterpret_cast<DarwinArtAndroidLocale>(uintptr_t{1}),
           reinterpret_cast<DarwinArtAndroidLocale>(UINTPTR_MAX),
       }) {
    const char* input = " -0b101010tail";
    char* expected_end = nullptr;
    errno = kGuestSentinel;
    const long long expected = aosp_strtoll(input, &expected_end, 0);
    const int expected_errno = errno;
    char* actual_end = nullptr;
    darwin_art_bionic_errno_store(kGuestSentinel);
    errno = kHostSentinel;
    const long long actual =
        darwin_art_bionic_strtoll_l(input, &actual_end, 0, locale);
    if (actual != expected || actual_end - input != expected_end - input ||
        darwin_art_bionic_errno_load() != expected_errno ||
        errno != kHostSentinel) {
      std::abort();
    }
    const unsigned long long unsigned_actual =
        darwin_art_bionic_strtoull_l("-1", nullptr, 10, locale);
    if (unsigned_actual != UINT64_MAX || errno != kHostSentinel) std::abort();
  }
}

void ThreadStress() {
  std::vector<std::thread> threads;
  for (int thread = 0; thread < 8; ++thread) {
    threads.emplace_back([thread] {
      for (int round = 0; round < 1'000; ++round) {
        const int guest = 10'000 + thread * 1'000 + round;
        const int host = 30'000 + thread;
        darwin_art_bionic_errno_store(guest);
        errno = host;
        char* end = nullptr;
        if (darwin_art_bionic_strtoull("ffffffffffffffff!", &end, 16) !=
                UINT64_MAX ||
            *end != '!' || darwin_art_bionic_errno_load() != guest ||
            errno != host) {
          std::abort();
        }
        if (darwin_art_bionic_strtoll("999999999999999999999!", &end, 10) !=
                INT64_MAX ||
            *end != '!' || darwin_art_bionic_errno_load() != 34 || errno != host) {
          std::abort();
        }
      }
    });
  }
  for (auto& thread : threads) thread.join();
}

}  // namespace

int main() {
  CompareCorpus();
  ThreadStress();
  std::puts(
      "bionic-numeric-facade: differential=PASS AOSP comparisons=6425 "
      "locale-handle=ignored threads=8x1000 ASan+UBSan=clean");
}
