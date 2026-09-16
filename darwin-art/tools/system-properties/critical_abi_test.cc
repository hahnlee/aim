// Include the original implementation only in this test so its private
// CriticalNative functions can be type-checked and invoked directly.
#include "android_os_SystemProperties.cpp"
#include <cassert>
#include <type_traits>
#include <iostream>

static_assert(std::is_same_v<decltype(&android::SystemProperties_get_integralH<jint>),
                             jint (*)(jlong, jint)>);
static_assert(std::is_same_v<decltype(&android::SystemProperties_get_integralH<jlong>),
                             jlong (*)(jlong, jlong)>);
static_assert(std::is_same_v<decltype(&android::SystemProperties_get_booleanH),
                             jboolean (*)(jlong, jboolean)>);

int main() {
  // Original libbase host store is solely the test backend. This does not
  // assert guest/native property-store integration or real ART dispatch.
  assert(android::base::SetProperty("test.critical.integer", "0x2a"));
  const auto integer = reinterpret_cast<jlong>(__system_property_find("test.critical.integer"));
  assert(integer != 0);
  assert(android::SystemProperties_get_integralH<jint>(integer, -1) == 42);
  assert(android::base::SetProperty("test.critical.integer", "9223372036854775807"));
  assert(android::SystemProperties_get_integralH<jlong>(integer, -1) == INT64_MAX);
  assert(android::SystemProperties_get_integralH<jint>(integer, -1) == -1);
  assert(android::base::SetProperty("test.critical.integer", "invalid"));
  assert(android::SystemProperties_get_integralH<jint>(integer, -9) == -9);
  assert(android::base::SetProperty("test.critical.boolean", "yes"));
  const auto boolean = reinterpret_cast<jlong>(__system_property_find("test.critical.boolean"));
  assert(android::SystemProperties_get_booleanH(boolean, JNI_FALSE) == JNI_TRUE);
  assert(android::base::SetProperty("test.critical.boolean", "off"));
  assert(android::SystemProperties_get_booleanH(boolean, JNI_TRUE) == JNI_FALSE);
  assert(android::base::SetProperty("test.critical.boolean", "unknown"));
  assert(android::SystemProperties_get_booleanH(boolean, JNI_TRUE) == JNI_TRUE);
  std::cout << "original SystemProperties CriticalNative ABI/parsing PASS\n";
}
