#pragma once

#include <string>

namespace darwin_art_acceptance {

struct Snapshot {
  bool network_elf_loaded = false;
  bool apk_elf_loaded = false;
  bool direct_apk_loaded = false;
  std::string apk_sha256;
  std::string apk_root_sha256;
};

void record_network_elf_loaded();
void record_apk_elf_loaded(std::string apk_sha256,
                           std::string apk_root_sha256);
void record_direct_apk_loaded();
Snapshot snapshot();

}  // namespace darwin_art_acceptance
