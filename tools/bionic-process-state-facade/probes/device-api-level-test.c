#include "aim_bionic_process_state.h"
#include <assert.h>
#include <stdio.h>
#include <string.h>

static const char* property;
int aim_bionic___system_property_get(const char* name, char* value) {
  assert(strcmp(name, "ro.build.version.sdk") == 0);
  if (property == NULL) { value[0] = '\0'; return 0; }
  const size_t length = strlen(property);
  assert(length < 92);
  memcpy(value, property, length + 1);
  return (int)length;
}
int main(void) {
  assert(aim_bionic_android_get_device_api_level() == -1);
  property = "35";
  assert(aim_bionic_android_get_device_api_level() == 35);
  property = "36";
  assert(aim_bionic_android_get_device_api_level() == 36);
  property = "0";
  assert(aim_bionic_android_get_device_api_level() == -1);
  property = "invalid";
  assert(aim_bionic_android_get_device_api_level() == -1);
  property = "-1";
  assert(aim_bionic_android_get_device_api_level() == -1);
  puts("original bionic device API property lookup PASS");
}
