#include "aim_bionic_float_conversion.h"

#include <errno.h>
#include <fenv.h>

#include <array>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <thread>
#include <type_traits>
#include <vector>

extern "C" double aim_aosp_strtod(const char*, char**);
extern "C" float aim_aosp_strtof(const char*, char**);
extern "C" int32_t aim_bionic_errno_load(void);
extern "C" void aim_bionic_errno_store(int32_t);

namespace {

constexpr int kGuestSentinel = 777;
constexpr int kHostSentinel = 31'941;

template <typename Value>
using Bits = std::conditional_t<sizeof(Value) == sizeof(uint64_t), uint64_t,
                                uint32_t>;

template <typename Value>
Bits<Value> ValueBits(Value value) {
  Bits<Value> bits = 0;
  static_assert(sizeof(bits) == sizeof(value));
  std::memcpy(&bits, &value, sizeof(bits));
  return bits;
}

template <typename Value>
void CompareOne(const char* name,
                const std::string& input,
                int rounding,
                Value (*reference)(const char*, char**),
                Value (*provider)(const char*, char**)) {
  if (fesetround(rounding) != 0) std::abort();
  feclearexcept(FE_ALL_EXCEPT);
  errno = 0;
  char* reference_end = nullptr;
  const Value reference_value = reference(input.c_str(), &reference_end);
  const int reference_errno = errno;
  const ptrdiff_t reference_offset = reference_end - input.c_str();

  if (fesetround(rounding) != 0) std::abort();
  feclearexcept(FE_ALL_EXCEPT);
  feraiseexcept(FE_DIVBYZERO);
  errno = kHostSentinel;
  aim_bionic_errno_store(kGuestSentinel);
  char* provider_end = nullptr;
  const Value provider_value = provider(input.c_str(), &provider_end);
  const int provider_errno = aim_bionic_errno_load();
  const ptrdiff_t provider_offset = provider_end - input.c_str();
  const int expected_guest_errno =
      reference_errno == ERANGE ? ERANGE : kGuestSentinel;
  if (ValueBits(provider_value) != ValueBits(reference_value) ||
      provider_offset != reference_offset ||
      provider_errno != expected_guest_errno || errno != kHostSentinel ||
      fegetround() != rounding || fetestexcept(FE_ALL_EXCEPT) != FE_DIVBYZERO) {
    std::fprintf(stderr,
                 "%s mismatch input='%s' rounding=%d bits=%llx/%llx "
                 "end=%td/%td errno=%d/%d host=%d exceptions=%x\n",
                 name, input.c_str(), rounding,
                 static_cast<unsigned long long>(ValueBits(provider_value)),
                 static_cast<unsigned long long>(ValueBits(reference_value)),
                 provider_offset, reference_offset, provider_errno,
                 expected_guest_errno, errno, fetestexcept(FE_ALL_EXCEPT));
    std::abort();
  }
}

void CompareCorpus() {
  const std::array<const char*, 47> inputs = {
      "",          " ",          "+",          "-",          "muppet",
      "  muppet",  "0",          "-0",         "0.",         ".0",
      ".",         "9.0",        "0.9e1",      "0x1.2p3",    "0x",
      "0xg",       "0x1p",       "0x1p+",      "1e",         "1e+",
      "1e+z",      "+inf",       "-infinity",  "infinitude", "NaN",
      "+nan",      "-nan(0xff)", "nanny",      "1e5000",     "1e-5000",
      "3.4028234663852886e38",     "1.1754943508222875e-38",
      "1.401298464324817e-45",     "7.0064923216240853546186479164495e-46",
      "7.0064923216240853546186479164496e-46",
      "1.7976931348623157e308",     "2.2250738585072012e-308",
      "4.9406564584124654e-324",     "9007199254740993",
      "-9007199254740993",          "0x1.fffffffffffffp1023",
      "0x0.0000000000001p-1022",    "0x1.fffffep127",
      "0x1p-149",                   "123_456", "\t\r\n42.5tail", "00000002.2250738585072012e-308"};
  const std::array<int, 4> roundings = {
      FE_TONEAREST, FE_DOWNWARD, FE_UPWARD, FE_TOWARDZERO};
  for (int rounding : roundings) {
    for (const char* input : inputs) {
      CompareOne("strtod", input, rounding, aim_aosp_strtod,
                 aim_bionic_strtod);
      CompareOne("strtof", input, rounding, aim_aosp_strtof,
                 aim_bionic_strtof);
    }
  }
}

void ThreadStress() {
  std::vector<std::thread> workers;
  for (int thread = 0; thread < 8; ++thread) {
    workers.emplace_back([thread] {
      for (int round = 0; round < 1'000; ++round) {
        const int host = 32'000 + thread;
        const int guest = 10'000 + thread * 1'000 + round;
        errno = host;
        aim_bionic_errno_store(guest);
        char* end = nullptr;
        if (ValueBits(aim_bionic_strtod("0x1.2p3!", &end)) !=
                UINT64_C(0x4022000000000000) ||
            *end != '!' || errno != host ||
            aim_bionic_errno_load() != guest) {
          std::abort();
        }
        if (ValueBits(aim_bionic_strtof("1e1000!", &end)) !=
                UINT32_C(0x7f800000) ||
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
  ThreadStress();
  std::puts(
      "bionic-float-conversion-differential: PASS AOSP-corpus=47x4x2 "
      "bits+end+errno rounding=4 host-fenv=preserved threads=8x1000 "
      "ASan+UBSan(no-shift)=clean");
  return 0;
}
