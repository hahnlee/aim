#include <bit>
#include <cstdint>
#include <cstdio>
#include <initializer_list>

using U64 = uint64_t;

extern "C" U64 register_direct_target(U64, U64, U64, U64, double, double);
extern "C" U64 darwin_to_android_thunk(
    U64, U64, U64, U64, U64, U64, U64, U64,
    double, double, double, double, double, double, double, double,
    uint8_t, int8_t, uint16_t, int16_t, int32_t, int64_t, uintptr_t, float, double);
extern "C" U64 android_callback_fixture(void* callback);

static U64 Mix(U64 accumulator, U64 value) {
  return std::rotr(accumulator ^ value, 7);
}

static U64 RegisterDigest(U64 a, U64 b, U64 c, U64 d, double e, double f) {
  U64 result = 0x6a09e667f3bcc909ULL;
  for (U64 value : {a, b, c, d, std::bit_cast<U64>(e), std::bit_cast<U64>(f)}) {
    result = Mix(result, value);
  }
  return result;
}

static U64 SpilledDigest(
    U64 a0, U64 a1, U64 a2, U64 a3, U64 a4, U64 a5, U64 a6, U64 a7,
    double d0, double d1, double d2, double d3,
    double d4, double d5, double d6, double d7,
    uint8_t z, int8_t b, uint16_t c, int16_t s, int32_t i, int64_t j,
    uintptr_t reference, float f, double d) {
  U64 result = 0x6a09e667f3bcc909ULL;
  for (U64 value : {a0, a1, a2, a3, a4, a5, a6, a7}) result = Mix(result, value);
  for (double value : {d0, d1, d2, d3, d4, d5, d6, d7}) {
    result = Mix(result, std::bit_cast<U64>(value));
  }
  result = Mix(result, z);
  result = Mix(result, static_cast<U64>(static_cast<int64_t>(b)));
  result = Mix(result, c);
  result = Mix(result, static_cast<U64>(static_cast<int64_t>(s)));
  result = Mix(result, static_cast<U64>(static_cast<int64_t>(i)));
  result = Mix(result, static_cast<U64>(j));
  result = Mix(result, reference);
  result = Mix(result, std::bit_cast<uint32_t>(f));
  result = Mix(result, std::bit_cast<U64>(d));
  return result;
}

extern "C" U64 darwin_callback_target(
    U64 a0, U64 a1, U64 a2, U64 a3, U64 a4, U64 a5, U64 a6, U64 a7,
    double d0, double d1, double d2, double d3,
    double d4, double d5, double d6, double d7,
    uint8_t z, int8_t b, uint16_t c, int16_t s, int32_t i, int64_t j,
    uintptr_t reference, float f, double d) {
  return SpilledDigest(a0, a1, a2, a3, a4, a5, a6, a7,
                       d0, d1, d2, d3, d4, d5, d6, d7,
                       z, b, c, s, i, j, reference, f, d);
}

int main() {
  U64 direct_expected = RegisterDigest(0x11, 0x22, 0x33, 0x44, 1.5, -9.25);
  U64 direct_actual = register_direct_target(0x11, 0x22, 0x33, 0x44, 1.5, -9.25);
  if (direct_actual != direct_expected) return 10;

  U64 spilled_expected = SpilledDigest(
      0x101, 0x202, 0x303, 0x404, 0x505, 0x606, 0x707, 0x808,
      1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0,
      1, -2, 0x3344, -1234, -1234567, 0x1122334455667788LL,
      0x123456789abcdef0ULL, 1.25f, -2.5);
  U64 forward = darwin_to_android_thunk(
      0x101, 0x202, 0x303, 0x404, 0x505, 0x606, 0x707, 0x808,
      1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0,
      1, -2, 0x3344, -1234, -1234567, 0x1122334455667788LL,
      0x123456789abcdef0ULL, 1.25f, -2.5);
  if (forward != spilled_expected) return 11;

  U64 reverse = android_callback_fixture(reinterpret_cast<void*>(&darwin_callback_target));
  if (reverse != spilled_expected) return 12;

  std::printf("register-direct=PASS digest=%016llx\n", direct_actual);
  std::printf("darwin-to-android-spill=PASS digest=%016llx\n", forward);
  std::printf("android-to-darwin-spill=PASS digest=%016llx\n", reverse);
  std::printf("pcs-scope=fixed-prototype nonvariadic registers=8+8 stack=ZBCSIJrefFD\n");
  return 0;
}

