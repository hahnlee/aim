#include "aim_bionic_wide_float.h"

#include <errno.h>
#include <fenv.h>

#include <array>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <string_view>
#include <thread>
#include <type_traits>
#include <vector>

#include <unicode/uchar.h>

extern "C" double aim_aosp_strtod(const char*, char**);
extern "C" float aim_aosp_strtof(const char*, char**);
extern "C" int32_t aim_bionic_errno_load(void);
extern "C" void aim_bionic_errno_store(int32_t);

namespace {

constexpr int kGuestSentinel = 779;
constexpr int kHostSentinel = 31'995;
constexpr std::string_view kAllowedAscii =
    "-+0123456789.xXeEpP()nNaAiIfFtTyY";

template <typename Value>
using Bits = std::conditional_t<sizeof(Value) == sizeof(uint64_t), uint64_t,
                                uint32_t>;

template <typename Value>
Bits<Value> ValueBits(Value value) {
  Bits<Value> bits = 0;
  std::memcpy(&bits, &value, sizeof(bits));
  return bits;
}

std::vector<uint32_t> Wide(std::string_view ascii) {
  std::vector<uint32_t> result;
  result.reserve(ascii.size() + 1);
  for (unsigned char byte : ascii) result.push_back(byte);
  result.push_back(0);
  return result;
}

bool Allowed(uint32_t code_point) {
  return code_point <= 0x7f &&
         kAllowedAscii.find(static_cast<char>(code_point)) !=
             std::string_view::npos;
}

template <typename Value>
struct ReferenceResult {
  Value value{};
  size_t end_offset = 0;
  int parser_errno = 0;
};

template <typename Value>
ReferenceResult<Value> Reference(const std::vector<uint32_t>& input,
                                 Value (*parser)(const char*, char**),
                                 int rounding) {
  size_t start = 0;
  while (u_hasBinaryProperty(static_cast<UChar32>(input[start]),
                             UCHAR_WHITE_SPACE)) {
    ++start;
  }
  size_t maximum_length = 0;
  while (Allowed(input[start + maximum_length])) ++maximum_length;
  std::string ascii;
  ascii.reserve(maximum_length);
  for (size_t index = 0; index < maximum_length; ++index) {
    ascii.push_back(static_cast<char>(input[start + index]));
  }
  if (fesetround(rounding) != 0) std::abort();
  feclearexcept(FE_ALL_EXCEPT);
  errno = 0;
  char* end = nullptr;
  const Value value = parser(ascii.c_str(), &end);
  const size_t consumed = static_cast<size_t>(end - ascii.c_str());
  return {value, consumed == 0 ? 0 : start + consumed, errno};
}

template <typename Value>
void CompareOne(const char* name,
                const std::vector<uint32_t>& input,
                int rounding,
                Value (*reference)(const char*, char**),
                Value (*provider)(const uint32_t*, uint32_t**)) {
  const ReferenceResult<Value> expected =
      Reference(input, reference, rounding);

  if (fesetround(rounding) != 0) std::abort();
  feclearexcept(FE_ALL_EXCEPT);
  feraiseexcept(FE_DIVBYZERO);
  errno = kHostSentinel;
  aim_bionic_errno_store(kGuestSentinel);
  uint32_t* end = nullptr;
  const Value actual = provider(input.data(), &end);
  const size_t actual_offset = static_cast<size_t>(end - input.data());
  const int expected_guest_errno =
      expected.parser_errno == ERANGE ? ERANGE : kGuestSentinel;
  if (ValueBits(actual) != ValueBits(expected.value) ||
      actual_offset != expected.end_offset ||
      aim_bionic_errno_load() != expected_guest_errno ||
      errno != kHostSentinel || fegetround() != rounding ||
      fetestexcept(FE_ALL_EXCEPT) != FE_DIVBYZERO) {
    std::fprintf(stderr,
                 "%s mismatch rounding=%d bits=%llx/%llx end=%zu/%zu "
                 "guest-errno=%d/%d host=%d exceptions=%x\n",
                 name, rounding,
                 static_cast<unsigned long long>(ValueBits(actual)),
                 static_cast<unsigned long long>(ValueBits(expected.value)),
                 actual_offset, expected.end_offset,
                 aim_bionic_errno_load(), expected_guest_errno, errno,
                 fetestexcept(FE_ALL_EXCEPT));
    std::abort();
  }
}

std::vector<std::vector<uint32_t>> Corpus() {
  constexpr std::array<const char*, 47> kAscii = {
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
      "0x1p-149",                   "123_456", "\t\r\n42.5tail",
      "00000002.2250738585072012e-308"};
  std::vector<std::vector<uint32_t>> result;
  result.reserve(58);
  for (const char* input : kAscii) result.push_back(Wide(input));
  for (uint32_t whitespace : {0x0085u, 0x00a0u, 0x1680u, 0x2003u, 0x2028u,
                              0x2029u, 0x202fu, 0x205fu, 0x3000u}) {
    std::vector<uint32_t> input = Wide("0x1.2p3tail");
    input.insert(input.begin(), whitespace);
    result.push_back(std::move(input));
  }
  result.push_back({0x200b, '4', '2', 0});
  result.push_back({'4', '2', '.', '5', 0x2603, '9', 0});
  if (result.size() != 58) std::abort();
  return result;
}

void CompareCorpus() {
  const auto corpus = Corpus();
  constexpr std::array<int, 4> kRoundings = {
      FE_TONEAREST, FE_DOWNWARD, FE_UPWARD, FE_TOWARDZERO};
  for (int rounding : kRoundings) {
    for (const auto& input : corpus) {
      CompareOne("wcstod", input, rounding, aim_aosp_strtod,
                 aim_bionic_wcstod);
      CompareOne("wcstof", input, rounding, aim_aosp_strtof,
                 aim_bionic_wcstof);
    }
  }
}

void ThreadStress() {
  std::vector<std::thread> workers;
  for (int thread = 0; thread < 8; ++thread) {
    workers.emplace_back([thread] {
      const std::vector<uint32_t> input = {0x2003, '0', 'x', '1', '.', '2',
                                           'p',    '3', '!', 0};
      for (int round = 0; round < 1'000; ++round) {
        const int host = 32'100 + thread;
        const int guest = 11'000 + thread * 1'000 + round;
        errno = host;
        aim_bionic_errno_store(guest);
        uint32_t* end = nullptr;
        if (ValueBits(aim_bionic_wcstod(input.data(), &end)) !=
                UINT64_C(0x4022000000000000) ||
            end - input.data() != 8 || errno != host ||
            aim_bionic_errno_load() != guest) {
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
      "bionic-wide-float-differential: PASS cases=58x4x2 "
      "Unicode-White_Space+nan+hex+overflow bits+end+errno "
      "threads=8x1000 host-errno+fenv=preserved ASan+UBSan(no-shift)=clean");
  return 0;
}
