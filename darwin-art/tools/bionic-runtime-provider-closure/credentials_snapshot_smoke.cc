#include "darwin_art_bionic_process_state.h"
#include <cstddef>
#include <cstdio>
#include <cstring>

static_assert(sizeof(DarwinArtProcessSnapshotConfigV2) == 152);
static_assert(offsetof(DarwinArtProcessSnapshotConfigV2, credentials) == 88);
static_assert(sizeof(DarwinArtProcessCredentialsConfig) == 64);
static_assert(offsetof(DarwinArtProcessCredentialsConfig, supplementary_groups) == 24);

extern "C" int darwin_art_credentials_snapshot_smoke() {
  uint32_t groups[] = {3003, 3009};
  DarwinArtProcessSnapshotConfigV2 config{};
  config.base.abi_version = 2;
  config.base.struct_size = sizeof(config);
  config.base.page_size = 16384;
  config.base.hwcap = 3;
  config.credentials = {10123, 10124, 10125, 20123, 20124, 20125,
                        groups, 2, 3, 1, 2};
  if (darwin_art_bionic_process_state_install_configured(&config.base) != 0) return 1;
  groups[0] = 999;
  config.credentials.uid = 999;
  DarwinArtProcessCredentialsOutput output;
  std::memset(&output, 0x5a, sizeof(output));
  unsigned char before[sizeof(output)];
  std::memcpy(before, &output, sizeof(output));
  uint32_t copy[] = {77, 88};
  int result = 0;
  if (darwin_art_bionic_process_state_read_credentials_core(&output, copy, 1) != -1 ||
      std::memcmp(&output, before, sizeof(output)) || copy[0] != 77 || copy[1] != 88) result = 2;
  if (darwin_art_bionic_process_state_read_credentials_core(&output, copy, 2) != 0 ||
      output.uid != 10123 || output.euid != 10124 || output.suid != 10125 ||
      output.gid != 20123 || output.egid != 20124 || output.sgid != 20125 ||
      output.supplementary_group_count != 2 || copy[0] != 3003 || copy[1] != 3009 ||
      output.permitted != 3 || output.effective != 1 || output.inheritable != 2) result = 3;
  config.credentials.effective = 4; // Not in the supplied permitted set.
  if (darwin_art_bionic_process_state_install_configured(&config.base) != -1) result = 4;
  if (darwin_art_bionic_process_state_read_credentials_core(&output, copy, 2) != 0 ||
      output.uid != 10123) result = 5;
  if (darwin_art_bionic_process_state_process_uninstall() != 0) result = 6;
  if (darwin_art_bionic_process_state_read_credentials_core(&output, copy, 2) != -1) result = 7;
  config.base.abi_version = 1;
  config.base.struct_size = sizeof(config.base);
  if (darwin_art_bionic_process_state_install_configured(&config.base) != 0) return 8;
  if (darwin_art_bionic_process_state_read_credentials_core(&output, copy, 2) != -1) result = 9;
  if (darwin_art_bionic_process_state_process_uninstall() != 0) result = 10;
  if (!result) std::fprintf(stderr, "credentials snapshot: C/Rust v2 copy, bounds, rollback, v1 absent PASS\n");
  return result;
}
