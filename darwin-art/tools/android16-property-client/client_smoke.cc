#include "darwin_art_bionic_process_state.h"
#include <cstdio>
#include <cstring>
extern "C" int darwin_art_aosp_system_property_set(const char*, const char*);
extern "C" int darwin_art_property_client_smoke(void) {
  if (darwin_art_bionic_process_state_process_install() != 0) return 1;
  char before[92]{}, after[92]{};
  int result = 0;
  if (darwin_art_bionic___system_property_get("ro.build.version.sdk", before) <= 0)
    result = 2;
  // No endpoint is installed: original client must fail without a local write.
  if (darwin_art_aosp_system_property_set(nullptr, "x") != -1) result = 3;
  if (darwin_art_aosp_system_property_set("ro.build.version.sdk", "999") == 0)
    result = 4;
  if (darwin_art_bionic___system_property_get("ro.build.version.sdk", after) <= 0 ||
      std::strcmp(before, after) != 0) result = 5;
  if (darwin_art_bionic_process_state_process_uninstall() != 0) result = 6;
  if (result == 0)
    std::fprintf(stderr, "original property client: absent service fails, read area unchanged PASS\n");
  return result;
}
