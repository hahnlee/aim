#pragma once

#include <cstddef>
#include <jni.h>

namespace art {
class ClassLinker;
class ScopedObjectAccess;
class Thread;
template <size_t kNumReferences>
class StackHandleScope;
}  // namespace art

namespace darwin_art_app {

// Local JNI references returned here are owned by the caller's local frame.
// The helper deliberately owns only the class-loading transaction; Activity,
// Window, and RenderNode objects remain in the orchestration TU so their
// Android lifetime is visible at the call site.
struct ClassSet {
  jobject app_loader = nullptr;
  jclass hello = nullptr;
  jclass activity = nullptr;
  jclass context = nullptr;
  jclass resources = nullptr;
  jclass view = nullptr;
  jclass canvas = nullptr;
  jclass content_root = nullptr;
  jclass package_manager = nullptr;
  jclass native_fixture = nullptr;
  jclass network_fixture = nullptr;
  jclass upstream_test_harness = nullptr;
};

int load_classes(JNIEnv* env,
                 art::Thread* self,
                 art::ClassLinker* class_linker,
                 art::ScopedObjectAccess& soa,
                 art::StackHandleScope<32>& hs,
                 bool run_apk_app,
                 const char* app_dex,
                 const char* support_dex,
                 const char* native_library_path,
                 const char* activity_descriptor,
                 bool run_direct_apk,
                 const char* direct_apk_path,
                 bool run_elf_jni_fixture,
                 bool run_network_acceptance,
                 bool headless_fixture,
                 ClassSet* out);

int load_native_library(JNIEnv* env, art::Thread* self, jobject app_loader,
                        const char* native_library_path);

// Extends the Java-constructed PathClassLoader's native search path when a
// package installs an additional converted Mach-O directory after startup.
int install_native_library_path(JNIEnv* env, jobject app_loader,
                                const char* native_library_path);

}  // namespace darwin_art_app
