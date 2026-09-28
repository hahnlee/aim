#include "runtime_abi_probe.h"

#include <cstdint>

namespace aim_abi_probe {

jlong packed_integer_stack(JNIEnv*, jclass, jint a0, jint a1, jint a2, jint a3,
                           jint a4, jint a5, jint spilled, jlong wide,
                           jint tail0, jint tail1) {
  return a0 == 10 && a1 == 11 && a2 == 12 && a3 == 13 && a4 == 14 &&
                 a5 == 15 && spilled == 0x10203040 &&
                 wide == INT64_C(0x1122334455667788) && tail0 == 0x50607080 &&
                 tail1 == 0x12345678
             ? static_cast<jlong>(UINT64_C(0x13579bdf2468ace0))
             : -1;
}

jlong packed_floating_stack(JNIEnv*, jclass, jfloat a0, jfloat a1, jfloat a2,
                             jfloat a3, jfloat a4, jfloat a5, jfloat a6,
                             jfloat a7, jfloat spilled, jdouble wide) {
  return a0 == 1.0f && a1 == 2.0f && a2 == 3.0f && a3 == 4.0f &&
                 a4 == 5.0f && a5 == 6.0f && a6 == 7.0f && a7 == 8.0f &&
                 spilled == 9.5f && wide == 10.25
             ? static_cast<jlong>(UINT64_C(0x02468ace13579bdf))
             : -1;
}

jlong packed_reference_stack(JNIEnv*, jclass, jint a0, jint a1, jint a2,
                              jint a3, jint a4, jint a5, jint spilled,
                              jobject reference, jint tail) {
  return a0 == 20 && a1 == 21 && a2 == 22 && a3 == 23 && a4 == 24 &&
                 a5 == 25 && spilled == 0x23456701 && reference != nullptr &&
                 tail == 0x34567812
             ? static_cast<jlong>(UINT64_C(0x55aa55aa33cc33cc))
             : -1;
}

jlong packed_narrow_stack(JNIEnv*, jclass, jint a0, jint a1, jint a2, jint a3,
                           jint a4, jint a5, jboolean bool_value,
                           jbyte byte_value, jchar char_value,
                           jshort short_value, jint int_value,
                           jlong long_value) {
  const bool valid =
      a0 == 30 && a1 == 31 && a2 == 32 && a3 == 33 && a4 == 34 && a5 == 35 &&
      bool_value == JNI_TRUE && static_cast<std::uint8_t>(byte_value) == 0x81 &&
      char_value == 0xabcd &&
      static_cast<std::uint16_t>(short_value) == 0x8765 &&
      int_value == 0x45678923 && long_value == INT64_C(0x2233445566778899);
  return valid ? static_cast<jlong>(UINT64_C(0x1122aabb3344ccdd)) : -1;
}

}  // namespace aim_abi_probe
