// JNI/transport doubles test context ownership only; not IPC or app acceptance.
#include "../compat/binder/context_manager.h"
#include <atomic>
#include <cassert>
#include <cstdio>
#include <cstring>
#include <thread>
#include <vector>

namespace {
thread_local bool pending = false;
bool fail_connection = true;
std::atomic<int> connections{0}, globals{0}, locals{0};
_jobject endpoint;
_jclass exception_class;
jboolean ExceptionCheck(JNIEnv*) { return pending; }
jclass FindClass(JNIEnv*, const char* name) {
  assert(std::strcmp(name, "android/os/RemoteException") == 0);
  return &exception_class;
}
jint ThrowNew(JNIEnv*, jclass, const char*) { pending = true; return JNI_OK; }
void DeleteLocalRef(JNIEnv*, jobject) {}
jobject NewGlobalRef(JNIEnv*, jobject object) { ++globals; return object; }
jobject NewLocalRef(JNIEnv*, jobject object) { ++locals; return object; }
}
namespace darwin_art {
jobject ConnectSystemBinder(JNIEnv*, const char*) {
  ++connections;
  return fail_connection ? nullptr : &endpoint;
}
}
int main() {
  JNINativeInterface table{};
  table.ExceptionCheck = ExceptionCheck;
  table.FindClass = FindClass;
  table.ThrowNew = ThrowNew;
  table.DeleteLocalRef = DeleteLocalRef;
  table.NewGlobalRef = NewGlobalRef;
  table.NewLocalRef = NewLocalRef;
  JNIEnv env{&table};
  assert(darwin_art::GetSystemContextObject(nullptr) == nullptr);
  pending = true;
  assert(darwin_art::GetSystemContextObject(&env) == nullptr);
  assert(connections == 0 && pending);
  pending = false;
  assert(darwin_art::GetSystemContextObject(&env) == nullptr);
  assert(connections == 1 && pending && globals == 0);
  pending = false;
  fail_connection = false;
  std::vector<std::thread> callers;
  for (int i = 0; i < 8; ++i) {
    callers.emplace_back([&table] {
      JNIEnv thread_env{&table};
      assert(darwin_art::GetSystemContextObject(&thread_env) == &endpoint);
      assert(!pending);
    });
  }
  for (auto& caller : callers) caller.join();
  assert(connections == 2 && globals == 1 && locals == 7);
  assert(darwin_art::GetSystemContextObject(&env) == &endpoint);
  assert(connections == 2 && globals == 1 && locals == 8);
  std::puts("Binder context ownership: PASS (failure, retry, concurrent single root)");
}
