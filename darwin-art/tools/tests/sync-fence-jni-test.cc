#include "window/sync_fence_jni.h"

#include <cassert>
#include <climits>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <limits>
#include <string>
#include <unordered_map>
#include <utility>

namespace {

struct Registration {
  std::string signature;
  void* function = nullptr;
};

struct FakeJni {
  std::unordered_map<std::string, Registration> methods;
  bool fail_find = false;
  jint register_result = JNI_OK;
};

FakeJni* active_jni = nullptr;
int close_calls = 0;
int last_closed_fd = -1;
int wait_calls = 0;
int last_wait_fd = -1;
int last_wait_timeout = -2;
int wait_result = 0;

jclass FindClass(JNIEnv*, const char* name) {
  assert(std::strcmp(name, "android/hardware/SyncFence") == 0);
  return active_jni->fail_find ? nullptr : reinterpret_cast<jclass>(1);
}

jint RegisterNatives(JNIEnv*, jclass, const JNINativeMethod* methods,
                     jint count) {
  if (active_jni->register_result != JNI_OK) return active_jni->register_result;
  assert(count == 7);
  for (jint i = 0; i < count; ++i) {
    active_jni->methods.emplace(
        methods[i].name,
        Registration{methods[i].signature, methods[i].fnPtr});
  }
  return JNI_OK;
}

void DeleteLocalRef(JNIEnv*, jobject) {}

template <typename Function>
Function Native(const FakeJni& jni, const char* name,
                const char* signature) {
  const auto found = jni.methods.find(name);
  assert(found != jni.methods.end());
  assert(found->second.signature == signature);
  return reinterpret_cast<Function>(found->second.function);
}

}  // namespace

extern "C" int darwin_art_bionic_socket_broker_close(int fd) {
  ++close_calls;
  last_closed_fd = fd;
  return 0;
}

extern "C" int sync_wait(int fd, int timeout_millis) {
  ++wait_calls;
  last_wait_fd = fd;
  last_wait_timeout = timeout_millis;
  return wait_result;
}

int main() {
  JNINativeInterface functions{};
  functions.FindClass = &FindClass;
  functions.RegisterNatives = &RegisterNatives;
  functions.DeleteLocalRef = &DeleteLocalRef;
  JNIEnv env{&functions};
  FakeJni jni;
  active_jni = &jni;

  assert(darwin_art::window::RegisterSyncFenceNatives(&env));
  assert(jni.methods.size() == 7);
  auto create = Native<jlong (*)(JNIEnv*, jclass, jint)>(
      jni, "nCreate", "(I)J");
  auto is_valid = Native<jboolean (*)(JNIEnv*, jclass, jlong)>(
      jni, "nIsValid", "(J)Z");
  auto get_fd = Native<jint (*)(JNIEnv*, jclass, jlong)>(
      jni, "nGetFd", "(J)I");
  auto wait = Native<jboolean (*)(JNIEnv*, jclass, jlong, jlong)>(
      jni, "nWait", "(JJ)Z");
  auto signal_time = Native<jlong (*)(JNIEnv*, jclass, jlong)>(
      jni, "nGetSignalTime", "(J)J");
  auto inc_ref = Native<void (*)(JNIEnv*, jclass, jlong)>(
      jni, "nIncRef", "(J)V");
  auto destructor = Native<jlong (*)(JNIEnv*, jclass)>(
      jni, "nGetDestructor", "()J");
  auto finalize = reinterpret_cast<void (*)(void*)>(destructor(&env, nullptr));

  assert(create(&env, nullptr, -1) == 0);
  assert(close_calls == 0);
  assert(is_valid(&env, nullptr, 0) == JNI_FALSE);
  assert(get_fd(&env, nullptr, 0) == -1);
  const int waits_before_invalid = wait_calls;
  assert(wait(&env, nullptr, 0, 1) == JNI_TRUE);
  assert(wait_calls == waits_before_invalid);
  assert(signal_time(&env, nullptr, 0) == -1);

  const jlong handle = create(&env, nullptr, 17);
  assert(handle != 0);
  assert(is_valid(&env, nullptr, handle) == JNI_TRUE);
  assert(get_fd(&env, nullptr, handle) == 17);

  wait_result = 0;
  const jlong max_millis_nanos =
      static_cast<jlong>(INT_MAX) * static_cast<jlong>(1000000);
  for (const auto& timeout : {
           std::pair<jlong, int>{0, 0},
           std::pair<jlong, int>{1, 1},
           std::pair<jlong, int>{999999, 1},
           std::pair<jlong, int>{1000000, 1},
           std::pair<jlong, int>{1000001, 2},
           std::pair<jlong, int>{-1, -1},
           std::pair<jlong, int>{max_millis_nanos - 999998, INT_MAX},
           std::pair<jlong, int>{max_millis_nanos, INT_MAX},
           std::pair<jlong, int>{max_millis_nanos + 1, INT_MAX},
           std::pair<jlong, int>{std::numeric_limits<jlong>::max(), INT_MAX},
       }) {
    assert(wait(&env, nullptr, handle, timeout.first) == JNI_TRUE);
    assert(last_wait_fd == 17 && last_wait_timeout == timeout.second);
  }

  wait_result = -1;
  assert(wait(&env, nullptr, handle, 0) == JNI_FALSE);
  assert(signal_time(&env, nullptr, handle) ==
         std::numeric_limits<jlong>::max());
  wait_result = 0;
  const jlong signaled_time = signal_time(&env, nullptr, handle);
  assert(signaled_time != -1 &&
         signaled_time != std::numeric_limits<jlong>::max());

  inc_ref(&env, nullptr, handle);
  finalize(reinterpret_cast<void*>(static_cast<uintptr_t>(handle)));
  assert(close_calls == 0);
  finalize(reinterpret_cast<void*>(static_cast<uintptr_t>(handle)));
  assert(close_calls == 1 && last_closed_fd == 17);

  jni.methods.clear();
  jni.register_result = JNI_ERR;
  assert(!darwin_art::window::RegisterSyncFenceNatives(&env));
  jni.register_result = JNI_OK;
  jni.fail_find = true;
  assert(!darwin_art::window::RegisterSyncFenceNatives(&env));

  std::puts("sync-fence-jni: PASS registration handles waits signal-time refcount");
}
