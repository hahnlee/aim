// Genuine ART/BinderProxy JNI-list fixture. The native remote actor is controlled;
// this does not prove wire obituaries, APK interaction, or reusable VM shutdown.
#include "runtime/art/vm_bootstrap.h"
#include "runtime/art/native_registration.h"
#include "runtime/art/process_state.h"
#include "compat/binder/proxy_death_recipient.h"
#include "compat/jni/scoped_local_frame.h"
#include "darwin_art_bionic_process_state.h"
#include "android_util_Binder.h"
#include <binder/Binder.h>
#include "interpreter/unstarted_runtime.h"
#include "scoped_thread_state_change-inl.h"
#include "thread-current-inl.h"
#include "well_known_classes.h"
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <crt_externs.h>
#include <unistd.h>
#include <vector>
#include <csignal>

namespace {
class RecipientActor final : public android::BBinder {
 public:
  android::BBinder* localBinder() override { return nullptr; }
  android::status_t linkToDeath(const android::sp<DeathRecipient>& recipient,
                               void* cookie, uint32_t flags) override {
    recipients_.push_back({recipient, cookie, flags});
    return android::NO_ERROR;
  }
  android::status_t unlinkToDeath(const android::wp<DeathRecipient>& recipient,
                                 void* cookie, uint32_t flags,
                                 android::wp<DeathRecipient>* removed) override {
    if (reject_unlink) return android::NAME_NOT_FOUND;
    for (auto it = recipients_.begin(); it != recipients_.end(); ++it) {
      if (it->recipient != recipient || it->cookie != cookie || it->flags != flags) continue;
      if (removed != nullptr) *removed = it->recipient;
      recipients_.erase(it);
      return android::NO_ERROR;
    }
    return android::NAME_NOT_FOUND;
  }
  bool reject_unlink = false;
 private:
  struct Link {
    android::wp<DeathRecipient> recipient;
    void* cookie;
    uint32_t flags;
  };
  std::vector<Link> recipients_;
};

bool InstallHostSnapshot() {
  const int page_size = getpagesize();
  if (page_size <= 0 || getuid() != geteuid() || getgid() != getegid()) return false;
  std::vector<DarwinArtProcessSnapshotEntry> environment;
  for (char** cursor = *_NSGetEnviron(); *cursor != nullptr; ++cursor) {
    const char* separator = std::strchr(*cursor, '=');
    if (separator == nullptr) return false;
    environment.push_back({reinterpret_cast<const uint8_t*>(*cursor),
        static_cast<size_t>(separator - *cursor),
        reinterpret_cast<const uint8_t*>(separator + 1), std::strlen(separator + 1)});
  }
  DarwinArtProcessSnapshotConfig snapshot{};
  snapshot.abi_version = 1;
  snapshot.struct_size = sizeof(snapshot);
  snapshot.environment = environment.data();
  snapshot.environment_count = environment.size();
  snapshot.page_size = static_cast<uint64_t>(page_size);
  // Only mandatory arm64 FP/ASIMD capabilities, matching the host launch owner.
  // No optional hardware feature is claimed by this scoped JNI-list fixture.
  snapshot.hwcap = 3;
  arc4random_buf(snapshot.random, sizeof(snapshot.random));
  return darwin_art_bionic_process_state_install_configured(&snapshot) == 0;
}

bool Check(JNIEnv* env, bool condition, const char* boundary) {
  if (condition && !env->ExceptionCheck()) return true;
  std::fprintf(stderr, "Binder recipient fixture FAIL: %s\n", boundary);
  if (env->ExceptionCheck()) env->ExceptionDescribe();
  return false;
}

int Run(JNIEnv* env) {
  darwin_art_jni_scope::ScopedLocalFrame frame(env);
  if (!frame.valid()) return 1;
  using android::DeathRecipientPresence;
  const auto actor = android::sp<RecipientActor>::make();
  jobject proxy = android::javaObjectForIBinder(env, actor);
  if (!Check(env, proxy != nullptr, "BinderProxy creation")) return 1;
  jclass proxy_type = env->FindClass("android/os/BinderProxy");
  if (!Check(env, proxy != nullptr && proxy_type != nullptr
      && env->IsInstanceOf(proxy, proxy_type), "actual BinderProxy factory")) return 1;
  jclass recipient_type = env->FindClass("dev/darwinart/tests/Recipient");
  if (!Check(env, recipient_type != nullptr, "fixture class loader")) return 1;
  jmethodID ctor = env->GetMethodID(recipient_type, "<init>", "()V");
  if (!Check(env, ctor != nullptr, "recipient constructor lookup")) return 1;
  jobject recipient = env->NewObject(recipient_type, ctor);
  if (!Check(env, recipient != nullptr, "recipient creation")) return 1;
  jmethodID link = env->GetMethodID(proxy_type, "linkToDeath",
      "(Landroid/os/IBinder$DeathRecipient;I)V");
  if (!Check(env, link != nullptr, "public link lookup")) return 1;
  jmethodID unlink = env->GetMethodID(proxy_type, "unlinkToDeath",
      "(Landroid/os/IBinder$DeathRecipient;I)Z");
  if (!Check(env, recipient != nullptr && link != nullptr && unlink != nullptr,
             "public link/unlink lookup")) return 1;
  env->CallVoidMethod(proxy, link, recipient, 0);
  if (!Check(env, android::QueryProxyDeathRecipient(env, proxy, recipient)
      == DeathRecipientPresence::PRESENT, "public link -> PRESENT")) return 1;

  actor->reject_unlink = true;
  env->CallBooleanMethod(proxy, unlink, recipient, 0);
  jthrowable failure = env->ExceptionOccurred();
  if (failure == nullptr) return Check(env, false, "failed unlink exception"), 1;
  env->ExceptionClear();
  jclass missing = env->FindClass("java/util/NoSuchElementException");
  if (!Check(env, missing != nullptr && env->IsInstanceOf(failure, missing)
      && android::QueryProxyDeathRecipient(env, proxy, recipient)
          == DeathRecipientPresence::PRESENT,
      "failed public unlink retains native recipient")) return 1;
  env->DeleteLocalRef(failure);
  actor->reject_unlink = false;
  const bool removed = env->CallBooleanMethod(proxy, unlink, recipient, 0);
  if (!Check(env, removed && android::QueryProxyDeathRecipient(env, proxy, recipient)
      == DeathRecipientPresence::ABSENT, "retry public unlink -> ABSENT")) return 1;

  jclass binder_type = env->FindClass("android/os/Binder");
  if (!Check(env, binder_type != nullptr, "local Binder lookup")) return 1;
  jmethodID binder_ctor = env->GetMethodID(binder_type, "<init>", "()V");
  if (!Check(env, binder_ctor != nullptr, "local Binder constructor")) return 1;
  jobject local = env->NewObject(binder_type, binder_ctor);
  if (!Check(env, local != nullptr && android::QueryProxyDeathRecipient(env, local, recipient)
      == DeathRecipientPresence::UNSUPPORTED, "local Binder -> UNSUPPORTED")) return 1;

  jclass error_type = env->FindClass("java/lang/IllegalStateException");
  if (!Check(env, error_type != nullptr, "pending exception lookup")) return 1;
  if (env->ThrowNew(error_type, "original fixture caller") != JNI_OK) return 1;
  jthrowable original = env->ExceptionOccurred();
  const auto result = android::QueryProxyDeathRecipient(env, proxy, recipient);
  jthrowable retained = env->ExceptionOccurred();
  env->ExceptionClear();
  if (!Check(env, result == DeathRecipientPresence::UNSUPPORTED && original != nullptr
      && retained != nullptr && env->IsSameObject(original, retained),
      "pending exception identity preserved")) return 1;
  std::puts("Genuine ART BinderProxy JNI-list PASS: link/present, failed unlink/retained, retry/absent, local/unsupported, pending exception identity");
  return 0;
}
}  // namespace

extern "C" __attribute__((visibility("default")))
int darwin_art_binder_recipient_test_entry(int argc, const char* const* argv) {
  if (argc != 5) return 64;
  @autoreleasepool {
    if (!InstallHostSnapshot()) return 66;
    if (!darwin_art_process::begin_run(nullptr)) return 65;
    darwin_art_process::ScopedRunBoundary boundary;
    darwin_art_process_config_t config{};
    config.struct_size = sizeof(config);
    config.core_oj_jar = argv[0];
    config.core_libart_jar = argv[1];
    config.framework_jar = argv[2];
    config.core_icu4j_jar = argv[3];
    darwin_art::embedding::ProcessConfigBounds bounds{64u << 20, 256u << 20};
    darwin_art::runtime_art::VmBootstrapResult vm;
    const int created = darwin_art::runtime_art::CreateVm(&config, bounds, argv[4], &vm);
    if (created != 0) return created;
    darwin_art_process::record_created_runtime(vm.self);
    boundary.set_art_thread(vm.self);
    art::interpreter::UnstartedRuntime::Initialize();
    art::ScopedObjectAccess soa(vm.self);
    art::WellKnownClasses::Init(vm.env);
    const int registered = darwin_art::runtime_art::StartNativeRegistration(vm.env, vm.self);
    if (registered != 0) {
      std::fprintf(stderr, "Binder recipient fixture: production native registration failed: %d\n", registered);
      if (vm.env->ExceptionCheck()) vm.env->ExceptionDescribe();
      return registered;
    }
    if (std::getenv("DARWIN_ART_TEST_FATAL_PLATFORM_SIGNAL") != nullptr) {
      if (std::atexit([] { std::fprintf(stderr, "FATAL_TEST_FINALIZER_RAN\n"); }) != 0)
        return 67;
      std::fprintf(stderr, "FATAL_TEST_REAL_ART_READY\n");
      std::raise(SIGABRT);
      return 68;  // A fatal handler must never return.
    }
    return Run(vm.env);
  }
}
