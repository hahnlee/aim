// JNI identity/lifetime doubles; this is not an ELF namespace visibility test.
#include "../compat/loader/classloader_identity.h"
#include <atomic>
#include <cassert>
#include <cstdio>
#include <thread>
#include <vector>

namespace {
struct Object : _jobject { int id; bool alive = true; explicit Object(int value) : id(value) {} };
std::atomic<int> created{0}, deleted{0};
bool fail_weak = false;
jboolean ExceptionCheck(JNIEnv*) { return false; }
jboolean IsSameObject(JNIEnv*, jobject a, jobject b) {
  if (b == nullptr) return a == nullptr || !static_cast<Object*>(a)->alive;
  return a != nullptr && static_cast<Object*>(a)->id == static_cast<Object*>(b)->id;
}
jweak NewWeakGlobalRef(JNIEnv*, jobject object) {
  if (fail_weak) return nullptr;
  ++created; return object;
}
void DeleteWeakGlobalRef(JNIEnv*, jweak) { ++deleted; }
}
int main() {
  JNINativeInterface functions{};
  functions.ExceptionCheck = ExceptionCheck; functions.IsSameObject = IsSameObject;
  functions.NewWeakGlobalRef = NewWeakGlobalRef; functions.DeleteWeakGlobalRef = DeleteWeakGlobalRef;
  JNIEnv env{&functions};
  using namespace darwin_art::loader;
  Object loader(1), alias(1), other(2);
  assert(EnsureClassLoaderIdentity(&env, nullptr) == 0);
  assert(FindClassLoaderIdentity(&env, &loader) == 0);
  std::vector<uint64_t> ids(8);
  std::vector<std::thread> threads;
  for (int i = 0; i < 8; ++i) threads.emplace_back([&, i] {
    JNIEnv thread_env{&functions};
    ids[i] = EnsureClassLoaderIdentity(&thread_env, &loader);
  });
  for (auto& thread : threads) thread.join();
  for (auto id : ids) assert(id != 0 && id == ids[0]);
  assert(created == 1);
  assert(EnsureClassLoaderIdentity(&env, &alias) == ids[0]);
  auto different = EnsureClassLoaderIdentity(&env, &other);
  assert(different > ids[0]);
  loader.alive = false;
  assert(FindClassLoaderIdentity(&env, &other) == different);
  assert(deleted == 1);
  // Reuse of a Java identity value after its old weak is cleared gets a new generation.
  auto reborn = EnsureClassLoaderIdentity(&env, &alias);
  assert(reborn > different);
  ReleaseClassLoaderIdentities(&env);
  assert(created == deleted);
  fail_weak = true;
  assert(EnsureClassLoaderIdentity(&env, &other) == 0);
  fail_weak = false;
  assert(EnsureClassLoaderIdentity(&env, &other) > reborn);
  ReleaseClassLoaderIdentities(&env);
  assert(created == deleted);
  std::puts("ClassLoader identity: PASS (concurrency, aliases, weak cleanup, non-reuse, allocation failure)");
}
