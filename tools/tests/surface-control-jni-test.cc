#include <jni.h>
#include <cassert>
#include <csignal>
#include <cstring>
#include <cstdio>
#include <dlfcn.h>
#include <sys/wait.h>
#include <unistd.h>

struct ASurfaceControl;
extern "C" ASurfaceControl* ASurfaceControl_fromJava(JNIEnv*, jobject);
namespace {
bool pending = false, wrong_type = false, missing_field = false;
jlong identity = 0x1234;
int retained = 0;
auto Convert = &ASurfaceControl_fromJava;
jclass Find(JNIEnv*, const char* name) {
  assert(std::strcmp(name, "android/view/SurfaceControl") == 0);
  return reinterpret_cast<jclass>(1);
}
jboolean Instance(JNIEnv*, jobject, jclass) { return !wrong_type; }
jfieldID Field(JNIEnv*, jclass, const char* name, const char* signature) {
  assert(std::strcmp(name, "mNativeObject") == 0 && std::strcmp(signature, "J") == 0);
  if (missing_field) { pending = true; return nullptr; }
  return reinterpret_cast<jfieldID>(2);
}
jlong Long(JNIEnv*, jobject, jfieldID) { return identity; }
jboolean Exception(JNIEnv*) { return pending; }
void Delete(JNIEnv*, jobject) {}
void Dies(JNIEnv* env, jobject object) {
  pid_t child = fork();
  assert(child >= 0);
  if (!child) { Convert(env, object); _exit(0); }
  int status = 0;
  assert(waitpid(child, &status, 0) == child);
  assert(WIFSIGNALED(status) && WTERMSIG(status) == SIGABRT);
}
}
extern "C" void ASurfaceControl_acquire(ASurfaceControl* control) {
  assert(reinterpret_cast<uintptr_t>(control) == static_cast<uintptr_t>(identity));
  ++retained;
}
int main(int argc, char** argv) {
  assert(argc == 1 || argc == 2);
  void* library = nullptr;
  void (*release)(ASurfaceControl*) = nullptr;
  ASurfaceControl* actual = nullptr;
  if (argc == 2) {
    library = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
    assert(library);
    Convert = reinterpret_cast<decltype(Convert)>(dlsym(library, "ASurfaceControl_fromJava"));
    auto create = reinterpret_cast<ASurfaceControl* (*)(ASurfaceControl*, const char*)>(
        dlsym(library, "ASurfaceControl_create"));
    release = reinterpret_cast<decltype(release)>(dlsym(library, "ASurfaceControl_release"));
    assert(Convert && create && release);
    actual = create(nullptr, "native-jni-ownership-test");
    assert(actual);
    identity = reinterpret_cast<uintptr_t>(actual);
  }
  JNINativeInterface functions{};
  functions.FindClass = Find; functions.IsInstanceOf = Instance;
  functions.GetFieldID = Field; functions.GetLongField = Long;
  functions.ExceptionCheck = Exception; functions.DeleteLocalRef = Delete;
  JNIEnv env{&functions};
  auto object = reinterpret_cast<jobject>(3);
  assert(reinterpret_cast<uintptr_t>(Convert(&env, object)) == static_cast<uintptr_t>(identity));
  const int expected_retains = library ? 0 : 1;
  assert(retained == expected_retains);
  pending = true;
  assert(!Convert(&env, object) && pending && retained == expected_retains);
  pending = false; missing_field = true;
  assert(!Convert(&env, object) && pending && retained == expected_retains);
  pending = false; missing_field = false;
  Dies(nullptr, object); Dies(&env, nullptr);
  wrong_type = true; Dies(&env, object); wrong_type = false;
  identity = 0; Dies(&env, object);
  if (library) {
    release(actual);  // Original owner.
    release(actual);  // Reference returned by conversion.
    assert(dlclose(library) == 0);
  }
  std::puts("SurfaceControl JNI: existing identity retained, exceptions preserved, invalid inputs fatal PASS");
}
