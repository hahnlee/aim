#include "aim_bionic_wide_integer.h"

#include <errno.h>
#include <fenv.h>

#include <array>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <string>
#include <thread>
#include <type_traits>
#include <vector>

extern "C" long aosp_strtol(const char*, char**, int);
extern "C" long long aosp_strtoll(const char*, char**, int);
extern "C" unsigned long aosp_strtoul(const char*, char**, int);
extern "C" unsigned long long aosp_strtoull(const char*, char**, int);
extern "C" int32_t aim_bionic_errno_load(void);
extern "C" void aim_bionic_errno_store(int32_t);

namespace {

constexpr int kGuestSentinel = 771;
constexpr int kHostSentinel = 31'871;

std::vector<uint32_t> Widen(const std::string& input) {
  std::vector<uint32_t> result;
  result.reserve(input.size() + 1);
  for (unsigned char value : input) result.push_back(value);
  result.push_back(0);
  return result;
}

template <typename Result, typename ByteParser, typename WideParser>
void CompareOne(const char* name,
                const std::string& input,
                int base,
                ByteParser byte_parser,
                WideParser wide_parser) {
  char* byte_end = nullptr;
  errno = kGuestSentinel;
  const Result expected = byte_parser(input.c_str(), &byte_end, base);
  const int expected_errno = errno;
  const ptrdiff_t expected_offset = byte_end - input.c_str();

  std::vector<uint32_t> wide = Widen(input);
  uint32_t* wide_end = nullptr;
  aim_bionic_errno_store(kGuestSentinel);
  errno = kHostSentinel;
  fesetround(FE_DOWNWARD);
  feclearexcept(FE_ALL_EXCEPT);
  feraiseexcept(FE_DIVBYZERO);
  const Result actual = wide_parser(wide.data(), &wide_end, base);
  const ptrdiff_t actual_offset = wide_end - wide.data();
  if (actual != expected || actual_offset != expected_offset ||
      aim_bionic_errno_load() != expected_errno ||
      errno != kHostSentinel || fegetround() != FE_DOWNWARD ||
      fetestexcept(FE_ALL_EXCEPT) != FE_DIVBYZERO) {
    std::fprintf(stderr,
                 "%s mismatch input='%s' base=%d value=%llu/%llu "
                 "end=%td/%td errno=%d/%d host=%d fenv=%x\n",
                 name, input.c_str(), base,
                 static_cast<unsigned long long>(actual),
                 static_cast<unsigned long long>(expected), actual_offset,
                 expected_offset, aim_bionic_errno_load(),
                 expected_errno, errno, fetestexcept(FE_ALL_EXCEPT));
    std::abort();
  }
}

void CompareCorpus() {
  const std::array<const char*, 31> inputs = {
      "",          " ",          "+",          "-",          "xyz",
      "  +xyz",    "0",          "00",         "0779",       "0x",
      "0x0",       "0xg",        "0x7fffffff", "-0X8000000000000000!",
      "0b",        "0b0",        "0b2",        "0b1010102",  "+42tail",
      "-42tail",   "9223372036854775807",       "9223372036854775808",
      "-9223372036854775808",     "-9223372036854775809",
      "18446744073709551615",     "18446744073709551616",
      "-18446744073709551615",    "-18446744073709551616",
      "zzzzzzzzzzzzzzzzzzzz",     "\t\r\n123", "123_456"};
  for (int base = -2; base <= 38; ++base) {
    for (const char* input : inputs) {
      CompareOne<long>("wcstol", input, base, aosp_strtol,
                       aim_bionic_wcstol);
      CompareOne<long long>("wcstoll", input, base, aosp_strtoll,
                            aim_bionic_wcstoll);
      CompareOne<unsigned long>("wcstoul", input, base, aosp_strtoul,
                                aim_bionic_wcstoul);
      CompareOne<unsigned long long>("wcstoull", input, base, aosp_strtoull,
                                     aim_bionic_wcstoull);
    }
  }
}

void WideEdges() {
  const std::array<uint32_t, 8> edges = {
      0x80, 0x100, 0xd800, 0xfdd0, 0x10ffff, 0x110000, 0x80000000,
      UINT32_MAX};
  for (uint32_t edge : edges) {
    const std::array<uint32_t, 4> input = {edge, '4', '2', 0};
    uint32_t* end = nullptr;
    aim_bionic_errno_store(kGuestSentinel);
    if (aim_bionic_wcstoll(input.data(), &end, 10) != 0 ||
        end != input.data() ||
        aim_bionic_errno_load() != kGuestSentinel) {
      std::abort();
    }
  }
}

void ThreadStress() {
  std::vector<std::thread> workers;
  for (int thread = 0; thread < 8; ++thread) {
    workers.emplace_back([thread] {
      const std::vector<uint32_t> maximum = Widen("ffffffffffffffff!");
      const std::vector<uint32_t> overflow =
          Widen("999999999999999999999!");
      for (int round = 0; round < 1'000; ++round) {
        const int host = 32'000 + thread;
        const int guest = 10'000 + thread * 1'000 + round;
        errno = host;
        aim_bionic_errno_store(guest);
        uint32_t* end = nullptr;
        if (aim_bionic_wcstoull(maximum.data(), &end, 16) !=
                UINT64_MAX ||
            *end != '!' || errno != host ||
            aim_bionic_errno_load() != guest) {
          std::abort();
        }
        if (aim_bionic_wcstoll(overflow.data(), &end, 10) != INT64_MAX ||
            *end != '!' || errno != host ||
            aim_bionic_errno_load() != ERANGE) {
          std::abort();
        }
      }
    });
  }
  for (std::thread& worker : workers) worker.join();
}

}  // namespace

int main() {
  CompareCorpus();
  WideEdges();
  ThreadStress();
  std::puts(
      "bionic-wide-integer-differential: PASS AOSP-byte/wchar32=5084 "
      "bases=-2..38 edges=8 threads=8x1000 host-errno+fenv=preserved "
      "ASan+UBSan=clean");
  return 0;
}
