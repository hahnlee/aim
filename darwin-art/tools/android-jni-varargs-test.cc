#include "jni/android_varargs.h"

#include <array>
#include <cassert>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <string>
#include <type_traits>
#include <vector>

#include <jni.h>

namespace {

using darwin_art::jni::AndroidArm64VaList;
using darwin_art::jni::DecodeAndroidArguments;

constexpr std::size_t kGpSlots = 8;
constexpr std::size_t kFpSlots = 8;

template <typename T>
void Store(std::uint8_t *destination, std::size_t offset, T value) {
  static_assert(std::is_trivially_copyable_v<T>);
  std::memcpy(destination + offset, &value, sizeof(value));
}

struct Fixture {
  alignas(16) std::array<std::uint8_t, kGpSlots * 8> gp{};
  alignas(16) std::array<std::uint8_t, kFpSlots * 16> fp{};
  alignas(16) std::array<std::uint8_t, 128> stack{};
  AndroidArm64VaList args{stack.data(), gp.data() + gp.size(),
                          fp.data() + fp.size(),
                          -static_cast<std::int32_t>(gp.size()),
                          -static_cast<std::int32_t>(fp.size())};

  void StoreGp(std::size_t slot, std::uint64_t value) {
    assert(slot < kGpSlots);
    Store(gp.data(), slot * 8, value);
  }

  void StoreFp(std::size_t slot, double value) {
    assert(slot < kFpSlots);
    std::memset(fp.data() + slot * 16, 0xa5, 16);
    Store(fp.data(), slot * 16, value);
  }

  void StoreStack(std::size_t slot, std::uint64_t value) {
    assert(slot * 8 + sizeof(value) <= stack.size());
    Store(stack.data(), slot * 8, value);
  }

  void StoreStackDouble(std::size_t slot, double value) {
    StoreStack(slot, 0);
    Store(stack.data(), slot * 8, value);
  }
};

void AssertVaListUnchanged(const AndroidArm64VaList &before,
                           const AndroidArm64VaList &after) {
  assert(before.stack == after.stack);
  assert(before.gr_top == after.gr_top);
  assert(before.vr_top == after.vr_top);
  assert(before.gr_offs == after.gr_offs);
  assert(before.vr_offs == after.vr_offs);
}

void AssertJvalueBytesEqual(const std::vector<jvalue> &expected,
                            const std::vector<jvalue> &actual) {
  assert(expected.size() == actual.size());
  if (!expected.empty()) {
    assert(std::memcmp(expected.data(), actual.data(),
                       expected.size() * sizeof(jvalue)) == 0);
  }
}

void ExpectRejected(const std::string &descriptor, Fixture *fixture) {
  assert(fixture != nullptr);
  const AndroidArm64VaList before = fixture->args;
  jvalue first{};
  first.j = static_cast<jlong>(0x1122334455667788ll);
  jvalue second{};
  second.l = reinterpret_cast<jobject>(static_cast<uintptr_t>(0x1234));
  const std::vector<jvalue> original{first, second};
  std::vector<jvalue> output = original;
  assert(!DecodeAndroidArguments(descriptor, &fixture->args, &output));
  AssertJvalueBytesEqual(original, output);
  AssertVaListUnchanged(before, fixture->args);
}

void TestFullRegisterAndOverflowLayout() {
  Fixture fixture;
  constexpr std::uint64_t kObject = 0x1111222233334444ull;
  constexpr std::uint64_t kArray = 0x5555666677778888ull;
  constexpr std::uint64_t kOverflowObject = 0x9999aaaabbbbccccull;
  constexpr std::uint64_t kOverflowArray = 0xddddeeeeffff0001ull;

  // Eight GP values occupy the GP save area, followed by eight independent FP
  // values in the 16-byte V-register save slots. The final five values use the
  // shared eight-byte overflow area.
  fixture.StoreGp(0, 0xfeedface00000001ull); // Z -> 1
  fixture.StoreGp(1, 0xfffffffffffffff9ull); // B -> -7
  fixture.StoreGp(2, 0xaaaa000000001234ull); // C -> 0x1234
  fixture.StoreGp(3, 0xbbbb0000fffffb2eull); // S -> -1234
  fixture.StoreGp(4, 0xcccccccc89abcdefull); // I -> 0x89abcdef
  fixture.StoreGp(5, 0x1122334455667788ull); // J
  fixture.StoreGp(6, kObject);               // Ljava/lang/Object;
  fixture.StoreGp(7, kArray);                // [I

  const double fp_values[kFpSlots] = {1.0 / 3.0, 2.25,  3.5,   4.75,
                                      5.125,     6.625, 7.875, 8.0625};
  for (std::size_t i = 0; i < kFpSlots; ++i) {
    fixture.StoreFp(i, fp_values[i]);
  }

  fixture.StoreStack(0, 0x8877665544332211ull); // J
  fixture.StoreStackDouble(1, 109.25);          // promoted F
  fixture.StoreStack(2, kOverflowObject);       // Ljava/lang/Object;
  fixture.StoreStackDouble(3, 110.5);           // D
  fixture.StoreStack(4, kOverflowArray);        // [[Ljava/lang/String;

  const std::string descriptor = "(ZBCSIJLjava/lang/Object;[I" +
                                 std::string(kFpSlots / 2, 'F') +
                                 std::string(kFpSlots / 2, 'D') +
                                 "JFLjava/lang/Object;D[[Ljava/lang/String;)V";
  const AndroidArm64VaList before = fixture.args;
  std::vector<jvalue> output;
  assert(DecodeAndroidArguments(descriptor, &fixture.args, &output));
  assert(output.size() == 21);
  assert(output[0].z == static_cast<jboolean>(1));
  assert(output[1].b == static_cast<jbyte>(-7));
  assert(output[2].c == static_cast<jchar>(0x1234));
  assert(output[3].s == static_cast<jshort>(-1234));
  assert(output[4].i == static_cast<jint>(0x89abcdef));
  assert(output[5].j == static_cast<jlong>(0x1122334455667788ll));
  assert(reinterpret_cast<uintptr_t>(output[6].l) == kObject);
  assert(reinterpret_cast<uintptr_t>(output[7].l) == kArray);
  for (std::size_t i = 0; i < kFpSlots / 2; ++i) {
    assert(output[8 + i].f == static_cast<jfloat>(fp_values[i]));
  }
  for (std::size_t i = 0; i < kFpSlots / 2; ++i) {
    assert(output[12 + i].d == fp_values[kFpSlots / 2 + i]);
  }
  assert(output[16].j == static_cast<jlong>(0x8877665544332211ll));
  assert(output[17].f == static_cast<jfloat>(109.25));
  assert(reinterpret_cast<uintptr_t>(output[18].l) == kOverflowObject);
  assert(output[19].d == 110.5);
  assert(reinterpret_cast<uintptr_t>(output[20].l) == kOverflowArray);
  AssertVaListUnchanged(before, fixture.args);
}

void TestMixedBankExhaustion() {
  Fixture fixture;
  for (std::size_t i = 0; i < kGpSlots; ++i) {
    fixture.StoreGp(i, 0x1000 + i);
  }
  for (std::size_t i = 0; i < kFpSlots; ++i) {
    fixture.StoreFp(i, 200.0 + i);
  }
  fixture.StoreStack(0, 0x2222333344445555ull); // GP overflow I
  fixture.StoreStack(1, 0xaaaabbbbccccddddull); // GP overflow J
  fixture.StoreStack(2, 0x1234);                // GP overflow C
  fixture.StoreStackDouble(3, 999.5);           // FP overflow D

  // Consume GP registers first, then spill one GP value while seven FP
  // registers remain. Finish the FP register bank, then exercise the shared
  // overflow stack after both banks are exhausted.
  std::string descriptor = "(" + std::string(kGpSlots, 'I') + "FI";
  descriptor += std::string(kFpSlots - 1, 'F');
  descriptor += "JCD)V";

  const AndroidArm64VaList before = fixture.args;
  std::vector<jvalue> output;
  assert(DecodeAndroidArguments(descriptor, &fixture.args, &output));
  assert(output.size() == 20);
  for (std::size_t i = 0; i < kGpSlots; ++i) {
    assert(output[i].i == static_cast<jint>(0x1000 + i));
  }
  assert(output[8].f == static_cast<jfloat>(200.0));
  assert(output[9].i == static_cast<jint>(0x2222333344445555ull));
  for (std::size_t i = 0; i < kFpSlots - 1; ++i) {
    assert(output[10 + i].f == static_cast<jfloat>(201.0 + i));
  }
  assert(output[17].j == static_cast<jlong>(0xaaaabbbbccccddddull));
  assert(output[18].c == static_cast<jchar>(0x1234));
  assert(output[19].d == 999.5);
  AssertVaListUnchanged(before, fixture.args);
}

void TestMalformedDescriptorsPreserveOutput() {
  Fixture fixture;
  fixture.StoreGp(0, 1);
  for (const std::string &descriptor : {
           std::string("(I"),       // missing ')'
           std::string("(I)X"),     // bad return
           std::string("(I)Vjunk"), // trailing junk
           std::string("(V)V"),     // V is not a parameter
           std::string("([V)V"),    // V is not an array component
           std::string("(L;)V"),    // empty object name
           std::string("I)V"),      // missing opening '('
       }) {
    ExpectRejected(descriptor, &fixture);
  }

  jvalue marker{};
  marker.i = 17;
  std::vector<jvalue> output{marker};
  assert(!DecodeAndroidArguments("()V", nullptr, &output));
  AssertJvalueBytesEqual({marker}, output);
  assert(!DecodeAndroidArguments("()V", &fixture.args, nullptr));
}

} // namespace

int main() {
  TestFullRegisterAndOverflowLayout();
  TestMixedBankExhaustion();
  TestMalformedDescriptorsPreserveOutput();
  std::puts("android-jni-varargs: PASS GP8 FP16 overflow8 promotion refs mixed "
            "malformed-preserve");
  return 0;
}
