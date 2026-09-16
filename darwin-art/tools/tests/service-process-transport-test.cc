#include "runtime/framework/os/service_process_transport.h"
#include <cassert>
#include <cerrno>
#include <cstring>
#include <fcntl.h>
#include <unistd.h>
#include <cstdio>
#include <cstdint>
#include <initializer_list>

namespace {
enum class Failure { None, String, Spawn, Send, Dispatcher, Array, Region } failure;
bool pending;
int fd = -1, releases, channels;
jint values[2];
using Spawn = jintArray (*)(JNIEnv*, jclass, jstring, jstring, jstring, jboolean, jobject);
using Release = jint (*)(JNIEnv*, jclass, jint, jint);
Spawn spawn;
Release release;
jboolean Check(JNIEnv*) { return pending; }
const char* Chars(JNIEnv*, jstring value, jboolean*) {
  assert(!pending);
  if (failure == Failure::String) { pending = true; return nullptr; }
  return reinterpret_cast<const char*>(value);
}
void ReleaseChars(JNIEnv*, jstring, const char*) {}
jclass Find(JNIEnv*, const char*) { return reinterpret_cast<jclass>(1); }
void Delete(JNIEnv*, jobject) {}
jint Register(JNIEnv*, jclass, const JNINativeMethod* methods, jint count) {
  for (jint i = 0; i < count; ++i) {
    if (std::strcmp(methods[i].name, "nativeSpawnService") == 0) spawn = reinterpret_cast<Spawn>(methods[i].fnPtr);
    if (std::strcmp(methods[i].name, "nativeReleaseRemoteService") == 0) release = reinterpret_cast<Release>(methods[i].fnPtr);
  }
  return JNI_OK;
}
jintArray NewArray(JNIEnv*, jsize size) {
  assert(size == 2);
  if (failure == Failure::Array) { pending = true; return nullptr; }
  return reinterpret_cast<jintArray>(values);
}
void SetRegion(JNIEnv*, jintArray, jsize start, jsize count, const jint* source) {
  assert(start == 0 && count == 2);
  if (failure == Failure::Region) { pending = true; return; }
  values[0] = source[0]; values[1] = source[1];
}
}
namespace darwin_art_process {
int32_t spawn_service_process(const char*, const char*, const char*, bool, int32_t* pid, int32_t* control) {
  if (failure == Failure::Spawn) return -1;
  fd = open("/dev/null", O_RDONLY | O_CLOEXEC);
  assert(fd >= 0);
  *pid = 4242; *control = fd;
  return 0;
}
int32_t release_service_process(int32_t pid) { assert(pid == 4242); ++releases; return 0; }
}
namespace darwin_art {
bool SendServiceBindIntent(JNIEnv*, jint, jobject) { return failure != Failure::Send; }
bool StartRemoteBinderDispatcher(JNIEnv*, jint) { return failure != Failure::Dispatcher; }
void CloseRemoteBinderChannel(JNIEnv*, jint control) { assert(control == fd); ++channels; }
bool RegisterRemoteBinderNatives(JNIEnv*, jclass) { return true; }
bool RegisterSystemServicesNatives(JNIEnv*, jclass) { return true; }
}
int main() {
  JNINativeInterface table{};
  table.ExceptionCheck = Check;
  table.GetStringUTFChars = Chars;
  table.ReleaseStringUTFChars = ReleaseChars;
  table.FindClass = Find;
  table.DeleteLocalRef = Delete;
  table.RegisterNatives = Register;
  table.NewIntArray = NewArray;
  table.SetIntArrayRegion = SetRegion;
  JNIEnv env{&table};
  assert(darwin_art::framework::os::RegisterServiceProcessTransport(&env, reinterpret_cast<jclass>(1)));
  assert(spawn && release);
  for (auto current : {Failure::String, Failure::Spawn, Failure::Send, Failure::Dispatcher, Failure::Array, Failure::Region, Failure::None}) {
    failure = current; pending = false; releases = channels = 0; fd = -1;
    auto text = reinterpret_cast<jstring>(const_cast<char*>("service"));
    auto result = spawn(&env, nullptr, text, text, text, JNI_FALSE, reinterpret_cast<jobject>(1));
    if (failure == Failure::None) {
      assert(result && !pending && releases == 0 && channels == 0);
      assert(values[0] == 4242 && values[1] == fd && fcntl(fd, F_GETFD) >= 0);
      assert(release(&env, nullptr, values[0], values[1]) == 0);
      assert(releases == 1 && channels == 1);
    } else {
      assert(!result);
      const bool before_spawn = failure == Failure::Spawn || failure == Failure::String;
      assert(releases == (before_spawn ? 0 : 1));
      assert(channels == (before_spawn ? 0 : 1));
      assert(pending == (failure == Failure::String || failure == Failure::Array || failure == Failure::Region));
    }
    if (fd >= 0) { errno = 0; assert(fcntl(fd, F_GETFD) == -1 && errno == EBADF); }
  }
  std::puts("service process transport: failure rollback, pending exceptions, successful ownership transfer PASS");
}
