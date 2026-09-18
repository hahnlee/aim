#pragma once

#include <string>

namespace darwin_art_process {

// Acceptance-only fixture paths and switches. Production APK/framework
// environment parsing belongs to runtime/embedding/process_config.h.
struct FixtureOptions final {
  std::string elf_fixture_path;
  std::string generic_elf_path;
  std::string apk_elf_path;
  std::string apk_sha256;
  std::string apk_root_sha256;
  std::string direct_apk_path;
  std::string direct_apk_root;
  std::string libcxx_collections_path;
  std::string libcxx_exception_path;
  std::string tls_fixture_path;
  std::string network_fixture_path;

  bool run_elf_jni_fixture = false;
  bool run_generic_elf = false;
  bool run_apk_elf = false;
  bool run_direct_apk = false;
  bool run_libcxx_acceptance = false;
  bool run_tls_acceptance = false;
  bool run_network_acceptance = false;
  bool run_framework_button = false;
  bool expect_apk_widgets = false;
};

// Returns 0 on success, 47 for an invalid mixed network mode, and 48 for
// malformed fixture arguments. `error` is diagnostic text only.
int LoadRuntimeFixtureOptions(FixtureOptions* options, std::string* error);

}  // namespace darwin_art_process
