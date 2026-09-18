#include "runtime_registration_fixture.h"

#include "../runtime/art/system_class_loader.h"

#include <iostream>
#include <cstdlib>
#include <cstring>

#include "darwin_framework_natives.h"
#include "darwin_icu_natives.h"
#include "darwin_libcore_natives.h"
#include "darwin_openjdk_natives.h"
#include "darwin_provider_owners.h"
#include "runtime_graphics_probe.h"
#include "graphics_fixture_state.h"
#include "jni/scoped_local_frame.h"
#include "../runtime/art/process_state.h"
#include "runtime.h"
#include "../runtime/framework/app/process_registration.h"
#include "gc/heap.h"
#include "thread-current-inl.h"
#include "thread-inl.h"
#include "mirror/throwable.h"

namespace darwin_art_registration_fixture {

namespace {
// Exercise the Java pre-close/dup2/final-close path more times than the broker
// has slots. Bind locally without connecting to an external service.
bool verify_socket_close(JNIEnv* env) {
  if (std::getenv("DARWIN_ART_VERIFY_SOCKET_CLOSE") == nullptr) return true;
  // This gate precedes app native-library loading, which normally acquires
  // the network provider. Give the isolated test its own balanced lease.
  std::string network_error;
  if (!darwin_art::providers::acquire_network(&network_error)) {
    std::cerr << "ART socket gate: " << network_error << "\n";
    return false;
  }
  struct NetworkLease {
    ~NetworkLease() { darwin_art::providers::release_network(); }
  } network_lease;
  if (env->PushLocalFrame(16) != JNI_OK) return false;
  jclass socket_class = env->FindClass("java/net/Socket");
  jclass address_class = env->FindClass("java/net/InetSocketAddress");
  if (env->ExceptionCheck()) { env->PopLocalFrame(nullptr); return false; }
  jmethodID socket_ctor = env->GetMethodID(socket_class, "<init>", "()V");
  jmethodID address_ctor = env->GetMethodID(address_class, "<init>", "(I)V");
  jmethodID bind = env->GetMethodID(socket_class, "bind", "(Ljava/net/SocketAddress;)V");
  jmethodID close = env->GetMethodID(socket_class, "close", "()V");
  if (env->ExceptionCheck()) { env->PopLocalFrame(nullptr); return false; }
  bool ok = true;
  int completed = 0;
  for (; completed < 1200; ++completed) {
    jobject socket = env->NewObject(socket_class, socket_ctor);
    jobject address = env->NewObject(address_class, address_ctor, 0);
    if (env->ExceptionCheck() || socket == nullptr || address == nullptr) {
      ok = false;
      break;
    }
    env->CallVoidMethod(socket, bind, address);
    jthrowable failure = env->ExceptionOccurred();
    if (completed == 0) std::cerr << "ART socket gate: bind exception=" << (failure != nullptr) << "\n";
    if (failure != nullptr) env->ExceptionClear();
    env->CallVoidMethod(socket, close);
    if (completed == 0) std::cerr << "ART socket gate: first close exception=" << static_cast<int>(env->ExceptionCheck()) << "\n";
    if (failure != nullptr) {
      env->ExceptionClear();
      env->Throw(failure);
      env->DeleteLocalRef(failure);
    }
    if (env->ExceptionCheck()) { ok = false; break; }
    // Java close is idempotent; this must not close a recycled broker token.
    env->CallVoidMethod(socket, close);
    if (env->ExceptionCheck()) { ok = false; break; }
    env->DeleteLocalRef(address);
    env->DeleteLocalRef(socket);
  }
  std::cerr << "ART Java socket close regression: " << (ok ? "PASS" : "FAIL")
            << " bind/pre-close/final-close/idempotence cycles=" << completed << "\n";
  if (!ok && env->ExceptionCheck()) env->ExceptionDescribe();
  env->PopLocalFrame(nullptr);
  return ok;
}

// Opt-in regression for the high-address moving collector. The ordinary APK
// path does not manufacture app objects or force a collection.
bool verify_compaction(JNIEnv* env) {
  if (std::getenv("DARWIN_ART_VERIFY_GC_COMPACTION") == nullptr) return true;
  if (env->PushLocalFrame(64) != JNI_OK) return false;
  struct LocalFrame {
    JNIEnv* env;
    ~LocalFrame() { env->PopLocalFrame(nullptr); }
  } local_frame{env};
  jclass object_class = env->FindClass("java/lang/Object");
  jclass system_class = env->FindClass("java/lang/System");
  jclass properties_class = env->FindClass("java/util/Properties");
  if (env->ExceptionCheck()) return false;
  jmethodID identity = env->GetStaticMethodID(system_class, "identityHashCode",
                                             "(Ljava/lang/Object;)I");
  jmethodID get_properties = env->GetStaticMethodID(system_class, "getProperties",
                                                   "()Ljava/util/Properties;");
  jmethodID put = env->GetMethodID(properties_class, "put",
      "(Ljava/lang/Object;Ljava/lang/Object;)Ljava/lang/Object;");
  jmethodID remove = env->GetMethodID(properties_class, "remove",
      "(Ljava/lang/Object;)Ljava/lang/Object;");
  jmethodID get = env->GetMethodID(properties_class, "get",
      "(Ljava/lang/Object;)Ljava/lang/Object;");
  if (env->ExceptionCheck()) return false;
  jobjectArray root = env->NewObjectArray(4, object_class, nullptr);
  jobjectArray child = env->NewObjectArray(1, object_class, nullptr);
  jstring value = env->NewStringUTF("darwin-art-moving-gc-roundtrip");
  jstring key = env->NewStringUTF("dev.darwinart.compaction.regression.root");
  jobject properties = env->CallStaticObjectMethod(system_class, get_properties);
  if (env->ExceptionCheck() || root == nullptr || child == nullptr ||
      value == nullptr || key == nullptr || properties == nullptr) return false;
  env->SetObjectArrayElement(root, 0, root);
  env->SetObjectArrayElement(root, 1, child);
  env->SetObjectArrayElement(root, 2, child);
  env->SetObjectArrayElement(root, 3, value);
  env->SetObjectArrayElement(child, 0, root);
  jobject global = env->NewGlobalRef(root);
  jweak weak = env->NewWeakGlobalRef(child);
  jobject previous = nullptr;
  bool property_installed = false;
  bool monitor_held = false;
  bool cleaned = false;
  auto cleanup = [&]() {
    if (cleaned) return true;
    cleaned = true;
    jthrowable pending = env->ExceptionOccurred();
    env->ExceptionClear();
    bool clean = pending == nullptr;
    if (monitor_held) {
      clean = env->MonitorExit(child) == JNI_OK && clean;
      if (env->ExceptionCheck()) { clean = false; env->ExceptionClear(); }
    }
    if (property_installed) {
      if (previous != nullptr) env->CallObjectMethod(properties, put, key, previous);
      else env->CallObjectMethod(properties, remove, key);
      if (env->ExceptionCheck()) { clean = false; env->ExceptionClear(); }
    }
    env->DeleteWeakGlobalRef(weak);
    env->DeleteGlobalRef(global);
    if (pending != nullptr) {
      env->Throw(pending);
      env->DeleteLocalRef(pending);
    }
    return clean;
  };
  struct CleanupGuard {
    decltype(cleanup)& callback;
    ~CleanupGuard() { callback(); }
  } cleanup_guard{cleanup};
  if (env->ExceptionCheck() || global == nullptr || weak == nullptr) return false;
  previous = env->CallObjectMethod(properties, put, key, root);
  if (env->ExceptionCheck()) return false;
  property_installed = true;
  const jint hash = env->CallStaticIntMethod(system_class, identity, child);
  if (env->ExceptionCheck()) return false;
  if (env->MonitorEnter(child) != JNI_OK) return false;
  monitor_held = true;
  const uintptr_t before = reinterpret_cast<uintptr_t>(
      art::Thread::Current()->DecodeJObject(root).Ptr());
  const auto result = art::Runtime::Current()->GetHeap()->PerformHomogeneousSpaceCompact();
  if (env->ExceptionCheck()) return false;
  const uintptr_t after = reinterpret_cast<uintptr_t>(
      art::Thread::Current()->DecodeJObject(root).Ptr());
  bool ok = result == art::gc::HomogeneousSpaceCompactResult::kSuccess && before != after;
  jobject self = env->GetObjectArrayElement(root, 0);
  jobject left = env->GetObjectArrayElement(root, 1);
  jobject right = env->GetObjectArrayElement(root, 2);
  jobject parent = env->GetObjectArrayElement(child, 0);
  jobject current_properties = env->CallStaticObjectMethod(system_class, get_properties);
  if (env->ExceptionCheck() || current_properties == nullptr) return false;
  jobject static_root = env->CallObjectMethod(current_properties, get, key);
  if (env->ExceptionCheck()) return false;
  jstring text = static_cast<jstring>(env->GetObjectArrayElement(root, 3));
  const char* utf = text == nullptr ? nullptr : env->GetStringUTFChars(text, nullptr);
  ok = ok && env->IsSameObject(self, root) && env->IsSameObject(parent, root) &&
       env->IsSameObject(left, right) && env->IsSameObject(left, weak) &&
       env->IsSameObject(properties, current_properties) &&
       env->IsSameObject(root, global) && env->IsSameObject(root, static_root) &&
       utf != nullptr &&
       std::strcmp(utf, "darwin-art-moving-gc-roundtrip") == 0 &&
       env->CallStaticIntMethod(system_class, identity, child) == hash;
  if (utf != nullptr) env->ReleaseStringUTFChars(text, utf);
  ok = cleanup() && ok;
  std::cerr << "ART moving GC regression: " << (ok ? "PASS" : "FAIL")
            << " cycles/shared/strings/hash/monitor/static/local/global/weak roots\n";
  return ok;
}
}  // namespace

int run_checks(JNIEnv* env) {
  if (env == nullptr) return 4;
  if (!verify_compaction(env)) return 42;
  if (!verify_socket_close(env)) return 43;
  return 0;
}

int finish(const Inputs& inputs) {
  if (inputs.env == nullptr || inputs.self == nullptr ||
      inputs.app_loader_ref == nullptr) {
    return 4;
  }
  JNIEnv* env = inputs.env;

  const int framework_status =
      darwin_art::framework::app::FinishFrameworkRegistration(env, true);
  if (framework_status != 0) return framework_status;

  // ActivityThread performs this after minimal runtime startup. Keep this
  // JNI-only bridge independent from the process/activity entry TU.
  if (darwin_art_install_context_loader(env, inputs.app_loader_ref) != 0) {
    return 4;
  }

  if (inputs.headless_fixture) {
    // Headless/runtime flavor intentionally has no GraphicsSession owner.
    // The real-graphics flavor supplies the state and installs the HWUI
    // canvas class; never manufacture a GPU owner in the CPU acceptance path.
    if (inputs.graphics_state != nullptr) {
      darwin_art_graphics_fixture::SetProbeCanvasClass(
          inputs.graphics_state, env, inputs.probe_canvas_class);
      if (env->ExceptionCheck()) {
        std::cerr << "ART Android window: ProbeCanvas global root failed\n";
        return 32;
      }
    }
  }

  return 0;
}

}  // namespace darwin_art_registration_fixture
