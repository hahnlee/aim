#include "aim_libm_facade.h"

#include <bit>
#include <cerrno>
#include <cfenv>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <initializer_list>

template <typename T, typename U>
static U Bits(T value) {
  return std::bit_cast<U>(value);
}

static void Require(bool condition) {
  if (!condition) std::abort();
}

int main() {
  using Fabs = double (*)(double);
  using Fabsf = float (*)(float);
  using CopySign = double (*)(double, double);
  using CopySignf = float (*)(float, float);
  auto facade_fabs = reinterpret_cast<Fabs>(aim_libm_resolve("fabs", "LIBC"));
  auto facade_fabsf = reinterpret_cast<Fabsf>(aim_libm_resolve("fabsf", nullptr));
  auto facade_copysign =
      reinterpret_cast<CopySign>(aim_libm_resolve("copysign", "LIBC"));
  auto facade_copysignf =
      reinterpret_cast<CopySignf>(aim_libm_resolve("copysignf", "LIBC"));
  Require(facade_fabs != nullptr && facade_fabsf != nullptr &&
          facade_copysign != nullptr && facade_copysignf != nullptr);
  Require(aim_libm_resolve("sqrt", "LIBC") == 0);
  Require(aim_libm_resolve("fabs", "GLIBC_2.0") == 0);
  Require(aim_libm_resolve("private_math", nullptr) == 0);

  const uint64_t doubles[] = {
      0x0000000000000000ULL, 0x8000000000000000ULL,
      0x7ff0000000000000ULL, 0xfff0000000000000ULL,
      0x7ff8deadbeef1234ULL, 0xfff8deadbeef1234ULL,
      0x7ff0000000000042ULL, 0xfff0000000000042ULL,
      0x0000000000000001ULL, 0x8000000000000001ULL,
  };
  const uint32_t floats[] = {
      0x00000000U, 0x80000000U, 0x7f800000U, 0xff800000U,
      0x7fc12345U, 0xffc12345U, 0x7f800042U, 0xff800042U,
      0x00000001U, 0x80000001U,
  };

  Fabs darwin_fabs = static_cast<Fabs>(&::fabs);
  Fabsf darwin_fabsf = static_cast<Fabsf>(&::fabsf);
  CopySign darwin_copysign = static_cast<CopySign>(&::copysign);
  CopySignf darwin_copysignf = static_cast<CopySignf>(&::copysignf);
  for (uint64_t bits : doubles) {
    double value = std::bit_cast<double>(bits);
    uint64_t expected = bits & 0x7fffffffffffffffULL;
    Require(Bits<double, uint64_t>(facade_fabs(value)) == expected);
    Require(Bits<double, uint64_t>(darwin_fabs(value)) == expected);
    for (uint64_t sign : {0ULL, 0x8000000000000000ULL}) {
      uint64_t copied = expected | sign;
      double sign_value = std::bit_cast<double>(sign);
      Require(Bits<double, uint64_t>(facade_copysign(value, sign_value)) == copied);
      Require(Bits<double, uint64_t>(darwin_copysign(value, sign_value)) == copied);
    }
  }
  for (uint32_t bits : floats) {
    float value = std::bit_cast<float>(bits);
    uint32_t expected = bits & 0x7fffffffU;
    Require(Bits<float, uint32_t>(facade_fabsf(value)) == expected);
    Require(Bits<float, uint32_t>(darwin_fabsf(value)) == expected);
    for (uint32_t sign : {0U, 0x80000000U}) {
      uint32_t copied = expected | sign;
      float sign_value = std::bit_cast<float>(sign);
      Require(Bits<float, uint32_t>(facade_copysignf(value, sign_value)) == copied);
      Require(Bits<float, uint32_t>(darwin_copysignf(value, sign_value)) == copied);
    }
  }

  for (int rounding : {FE_TONEAREST, FE_DOWNWARD, FE_UPWARD, FE_TOWARDZERO}) {
    Require(std::fesetround(rounding) == 0);
    std::feclearexcept(FE_ALL_EXCEPT);
    std::feraiseexcept(FE_DIVBYZERO);
    errno = EDOM;
    volatile double result = facade_copysign(facade_fabs(-0.0), -1.0);
    (void)result;
    Require(errno == EDOM);
    Require(std::fetestexcept(FE_ALL_EXCEPT) == FE_DIVBYZERO);
  }
  std::fesetround(FE_TONEAREST);
  std::feclearexcept(FE_ALL_EXCEPT);

  Require(aim_libm_capability("fabs") == AIM_LIBM_BIT_EXACT);
  Require(aim_libm_capability("floor") ==
          AIM_LIBM_RESULT_ONLY_FENV_UNPROVEN);
  Require(aim_libm_capability("sqrt") == AIM_LIBM_ERRNO_OR_FENV_SENSITIVE);
  Require(aim_libm_capability("sqrtl") == AIM_LIBM_UNSUPPORTED_ABI);
  Require(aim_libm_capability("csqrt") == AIM_LIBM_UNSUPPORTED_ABI);

  std::puts("android-libm-facade: PASS safe=4 nan-payload=preserved signed-zero=preserved");
  std::puts("android-libm-facade: errno=unchanged fenv=unchanged rounding-mode=independent");
  std::puts("android-libm-facade: blocked=rounding-unproven,errno-fenv-sensitive,long-double,complex");
  return 0;
}
