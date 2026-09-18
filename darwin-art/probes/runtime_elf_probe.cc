#include "runtime_elf_probe.h"
#include "runtime_acceptance_state.h"
#include "elf_fixture_journal.h"

#include <cstdint>
#include <iostream>

#include "darwin_provider_owners.h"
#include "scoped_thread_state_change-inl.h"
#include "thread-current-inl.h"
#include "jni/java_vm_ext.h"
#include "runtime.h"
#include "well_known_classes.h"

namespace android {
enum JNICallType {
  kJNICallTypeRegular = 1,
};
extern "C" void* NativeBridgeGetTrampoline2(void* handle, const char* name,
                                             const char* shorty, uint32_t len,
                                             JNICallType jni_call_type);
extern "C" void* OpenNativeLibrary(JNIEnv* env, int32_t target_sdk_version,
                                    const char* path, jobject class_loader,
                                    const char* caller_location,
                                    jstring library_path,
                                    bool* needs_native_bridge,
                                    char** error_msg);
extern "C" bool CloseNativeLibrary(void* handle, bool needs_native_bridge,
                                    char** error_msg);
extern "C" void NativeLoaderFreeErrorMessage(char* message);
}  // namespace android

namespace darwin_art_elf_probe {

bool run_android_elf_self_test(JNIEnv* env, JavaVM* vm, jobject class_loader,
                               const char* path, std::string* error) {
  bool needs_native_bridge = false;
  char* open_error = nullptr;
  void* handle = android::OpenNativeLibrary(
      env, 35, path, class_loader, nullptr, nullptr, &needs_native_bridge,
      &open_error);
  if (handle == nullptr || open_error != nullptr || !needs_native_bridge) {
    *error = open_error == nullptr ? "Android ELF open failed without detail"
                                   : open_error;
    android::NativeLoaderFreeErrorMessage(open_error);
    if (handle != nullptr) {
      char* close_error = nullptr;
      (void)android::CloseNativeLibrary(handle, needs_native_bridge,
                                        &close_error);
      android::NativeLoaderFreeErrorMessage(close_error);
    }
    return false;
  }
  android::NativeLoaderFreeErrorMessage(open_error);

  void* entry = android::NativeBridgeGetTrampoline2(
      handle, "JNI_OnLoad", nullptr, 0, android::kJNICallTypeRegular);
  using JniOnLoad = jint (*)(JavaVM*, void*);
  const jint version = entry == nullptr
                           ? JNI_ERR
                           : reinterpret_cast<JniOnLoad>(entry)(vm, nullptr);

  // JavaVMExt invokes JNI_OnUnload before releasing a NativeBridge image.
  // The self-test calls JNI_OnLoad directly through the same trampoline, so
  // mirror that lifecycle edge explicitly instead of asking the owner
  // teardown to reclaim a live JNI registration set.
  void* unload_entry = android::NativeBridgeGetTrampoline2(
      handle, "JNI_OnUnload", nullptr, 0, android::kJNICallTypeRegular);
  using JniOnUnload = void (*)(JavaVM*, void*);
  if (unload_entry != nullptr) {
    reinterpret_cast<JniOnUnload>(unload_entry)(vm, nullptr);
  }

  char* close_error = nullptr;
  const bool closed =
      android::CloseNativeLibrary(handle, needs_native_bridge, &close_error);
  if (version != JNI_VERSION_1_6 || !closed || close_error != nullptr) {
    *error = close_error == nullptr
                 ? "self-testing JNI_OnLoad returned " +
                       std::to_string(static_cast<int>(version))
                 : close_error;
    android::NativeLoaderFreeErrorMessage(close_error);
    return false;
  }
  android::NativeLoaderFreeErrorMessage(close_error);
  return true;
}

namespace {
int close_native_library(void* handle, bool needs_native_bridge) {
  char* close_error = nullptr;
  const bool closed =
      android::CloseNativeLibrary(handle, needs_native_bridge, &close_error);
  android::NativeLoaderFreeErrorMessage(close_error);
  return closed ? 0 : -1;
}
}  // namespace

int run_fixture_graph_acceptance(const FixtureGraphAcceptance& input) {
  if (!input.run_generic_elf || !input.run_apk_elf ||
      !input.run_libcxx_acceptance || !input.run_tls_acceptance) {
    std::cerr << "ART Android ELF fixture graph paths are incomplete\n";
    return input.run_generic_elf ? 43 : 40;
  }

  std::string libcxx_error;
  bool collections_ok = false;
  bool exception_ok = false;
  {
    art::ScopedThreadSuspension suspended(input.self, art::ThreadState::kNative);
    JavaVM* vm = reinterpret_cast<JavaVM*>(art::Runtime::Current()->GetJavaVM());
    collections_ok = run_android_elf_self_test(
        input.env, vm, input.app_loader_ref, input.libcxx_collections_path,
        &libcxx_error);
    if (collections_ok) {
      exception_ok = run_android_elf_self_test(
          input.env, vm, input.app_loader_ref, input.libcxx_exception_path,
          &libcxx_error);
    }
  }
  if (!collections_ok || !exception_ok || input.env->ExceptionCheck()) {
    std::cerr << "ART Android libc++ acceptance failed: " << libcxx_error
              << "\n";
    return 43;
  }
  std::cout << "ART Android libc++: real-r28c collections=189 "
               "exception-cleanup=73 unload=sequential\n"
            << std::flush;

  std::string tls_error;
  bool tls_ok = false;
  {
    art::ScopedThreadSuspension suspended(input.self, art::ThreadState::kNative);
    JavaVM* vm = reinterpret_cast<JavaVM*>(art::Runtime::Current()->GetJavaVM());
    tls_ok = run_android_elf_self_test(input.env, vm, input.app_loader_ref,
                                       input.tls_fixture_path, &tls_error);
  }
  if (!tls_ok || input.env->ExceptionCheck()) {
    std::cerr << "ART Android ELF TLS acceptance failed: " << tls_error << "\n";
    return 44;
  }
  std::cout << "ART Android ELF TLS: local-TLSDESC threads=4 align=64 "
               "unload=quiescent\n"
            << std::flush;

  std::string generic_load_error;
  bool generic_loaded = false;
  {
    art::ScopedThreadSuspension suspended(input.self, art::ThreadState::kNative);
    generic_loaded =
        art::Runtime::Current()->GetJavaVM()->LoadNativeLibrary(
            input.env, input.generic_elf_path, input.app_loader_ref, nullptr,
            &generic_load_error);
  }
  if (!generic_loaded || !generic_load_error.empty() ||
      input.env->ExceptionCheck()) {
    std::cerr << "ART Android ELF generic graph load failed, load_error="
              << generic_load_error << "\n";
    return 40;
  }

  jmethodID generic_native_add = input.env->GetStaticMethodID(
      input.native_fixture_class, "nativeAdd", "(IJI)J");
  const jlong generic_add_result =
      generic_native_add == nullptr
          ? -1
          : input.env->CallStaticLongMethod(input.native_fixture_class,
                                             generic_native_add, 10, jlong{20},
                                             12);
  if (generic_add_result != 42 || input.env->ExceptionCheck()) {
    std::cerr << "ART Android ELF generic RegisterNatives failed, result="
              << generic_add_result << "\n";
    return 40;
  }

  darwin_art_acceptance::record_apk_elf_loaded(input.apk_sha256,
                                                input.apk_root_sha256);
  if (!ResetJournal()) return 40;
  char* partial_error = nullptr;
  void* partial_handle = android::OpenNativeLibrary(
      input.env, 35, input.elf_fixture_path, input.app_loader_ref, nullptr,
      nullptr, nullptr, &partial_error);
  const bool partial_cleanup_ok =
      partial_handle == nullptr && partial_error != nullptr &&
      ReadJournal() == "124567";
  if (partial_handle != nullptr) {
    close_native_library(partial_handle, true);
  }
  const std::string partial_error_text =
      partial_error == nullptr ? "<none>" : partial_error;
  android::NativeLoaderFreeErrorMessage(partial_error);
  if (!partial_cleanup_ok || input.env->ExceptionCheck()) {
    std::cerr << "ART Android ELF JNI: partial failure cleanup failed, lifecycle="
              << ReadJournal()
              << " error=" << partial_error_text << "\n";
    return 40;
  }

  if (!ResetJournal()) return 41;
  std::string load_error;
  bool loaded = false;
  {
    art::ScopedThreadSuspension suspended(input.self, art::ThreadState::kNative);
    loaded = art::Runtime::Current()->GetJavaVM()->LoadNativeLibrary(
        input.env, input.elf_fixture_path, input.app_loader_ref,
        input.native_fixture_class, &load_error);
  }
  if (!loaded || !load_error.empty() || ReadJournal() != "123" ||
      input.env->ExceptionCheck()) {
    std::cerr << "ART Android ELF JNI: load/registration failed, status="
              << (loaded ? "loaded" : "failed") << " lifecycle=" << ReadJournal()
              << " load_error=" << load_error << "\n";
    return 41;
  }
  jmethodID run_acceptance = input.env->GetStaticMethodID(
      input.native_fixture_class, "runAcceptance", "()I");
  const jint acceptance =
      run_acceptance == nullptr
          ? -3
          : input.env->CallStaticIntMethod(input.native_fixture_class,
                                            run_acceptance);
  if (acceptance != 42 || input.env->ExceptionCheck()) {
    std::cerr << "ART Android ELF JNI: nativeAdd/nativeSpill failed, result="
              << acceptance << "\n";
    return 42;
  }
  std::cout << "ART Android ELF JNI: graph=child-first+relocated "
               "providers=bind_builtins+__errno+strlen+fs-random-ctor+scanf+"
               "swprintf+ioctl+strftime+sendfile "
               "load+JNI_OnLoad+RegisterNatives=generic+fixture scalar-ref=all "
               "nativeUsesEnv=current stack-repack=ok\n"
            << std::flush;
  return 0;
}

}  // namespace darwin_art_elf_probe
