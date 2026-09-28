#include "aim_bionic_process_state.h"
#include <cassert>
#include <cstdio>
#include <cstring>

static_assert(sizeof(AimProcessSnapshotConfig) == 88);
static_assert(offsetof(AimProcessSnapshotConfig, random) == 72);
static_assert(sizeof(AimProcessSnapshotEntry) == 32);

extern "C" int aim_configured_snapshot_smoke() {
  char name[] = "runtime.configured";
  char value[] = "from-authority";
  const AimProcessSnapshotEntry entry{
      reinterpret_cast<const uint8_t*>(name), sizeof(name) - 1,
      reinterpret_cast<const uint8_t*>(value), sizeof(value) - 1};
  AimProcessSnapshotConfig config{};
  config.abi_version = 1;
  config.struct_size = sizeof(config);
  config.properties = &entry;
  config.property_count = 1;
  config.page_size = 16384;
  config.hwcap = 3;
  assert(aim_bionic_process_state_is_installed() == 0);
  assert(aim_bionic_process_state_install_configured(nullptr) == -1);
  assert(aim_bionic_process_state_install_configured(&config) == 0);
  assert(aim_bionic_process_state_is_installed() == 1);
  std::memset(name, 'x', sizeof(name) - 1);
  std::memset(value, 'x', sizeof(value) - 1);
  char result[92]{};
  assert(aim_bionic___system_property_get("runtime.configured", result) == 14);
  assert(std::strcmp(result, "from-authority") == 0);
  assert(aim_bionic___system_property_find("device.cpu.count") == nullptr);
  assert(aim_bionic_process_state_install_configured(&config) == -1);
  assert(aim_bionic_process_state_is_installed() == 1);
  assert(aim_bionic_process_state_process_uninstall() == 0);
  assert(aim_bionic_process_state_is_installed() == 0);
  std::puts("configured process snapshot: C/Rust ABI, copied inputs, no fabricated defaults, duplicate rejection PASS");
  return 0;
}
