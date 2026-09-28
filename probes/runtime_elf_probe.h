#pragma once

#include <jni.h>

#include <string>

namespace art {
class Thread;
}

namespace aim_elf_probe {

// Owns the inputs for the fixture graph acceptance phase.  Keeping this
// phase in its own translation unit means changes to the ART process
// bootstrap no longer rebuild the ELF/JNI acceptance object and keeps the
// process entrypoint focused on lifecycle orchestration.
struct FixtureGraphAcceptance {
  JNIEnv* env;
  art::Thread* self;
  jobject app_loader_ref;
  jclass native_fixture_class;
  const char* elf_fixture_path;
  const char* generic_elf_path;
  const char* libcxx_collections_path;
  const char* libcxx_exception_path;
  const char* tls_fixture_path;
  const char* apk_sha256;
  const char* apk_root_sha256;
  bool run_generic_elf;
  bool run_apk_elf;
  bool run_libcxx_acceptance;
  bool run_tls_acceptance;
};

// Exercises the Android NativeLoader/NativeBridge path without making the
// process orchestration TU own ELF graph policy.
bool run_android_elf_self_test(JNIEnv* env, JavaVM* vm, jobject class_loader,
                               const char* path, std::string* error);

// Returns the historical process-probe status code (0 on success).
int run_fixture_graph_acceptance(const FixtureGraphAcceptance& input);

}  // namespace aim_elf_probe
