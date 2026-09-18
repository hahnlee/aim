#include "runtime/framework/os/service_process_transport.h"
#include <bit>
#include <cassert>
#include <cerrno>
#include <cstring>
#include <fcntl.h>
#include <unistd.h>
#include <cstdio>
#include <cstdint>
#include <initializer_list>
#include <memory>

#include "compat/binder/wire_channel_lifetime.h"
#include "compat/process/host_services.h"

namespace darwin_art::binder {

// Test-only access keeps the production lifetime API free of a reset or
// injection hook while allowing this ABI fixture to exercise Java's signed
// jlong representation of a high-bit generation.
class WireChannelLifetimeTestPeer final {
 public:
  static void SetNextGeneration(uint64_t generation) noexcept {
    WireChannelLifetime::next_generation_.store(generation,
                                                  std::memory_order_relaxed);
  }
};

}  // namespace darwin_art::binder

namespace {
enum class Failure {
  None,
  String,
  Spawn,
  Send,
  Dispatcher,
  Revoked,
  Array,
  Region,
} failure;
bool pending;
int fd = -1, releases, channels;
jlong values[3];
bool capture_called, dispatcher_called;
std::shared_ptr<darwin_art::binder::WireChannelLifetime> fixture_lifetime;
uint64_t captured_generation;
using Spawn = jlongArray (*)(JNIEnv*, jclass, jstring, jstring, jstring, jboolean, jobject);
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
    if (std::strcmp(methods[i].name, "nativeSpawnService") == 0) {
      assert(std::strcmp(methods[i].signature,
          "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;ZLandroid/content/Intent;)[J") == 0);
      spawn = reinterpret_cast<Spawn>(methods[i].fnPtr);
    }
    if (std::strcmp(methods[i].name, "nativeReleaseRemoteService") == 0) {
      assert(std::strcmp(methods[i].signature, "(II)I") == 0);
      release = reinterpret_cast<Release>(methods[i].fnPtr);
    }
  }
  return JNI_OK;
}
jlongArray NewArray(JNIEnv*, jsize size) {
  assert(size == 3);
  if (failure == Failure::Array) { pending = true; return nullptr; }
  return reinterpret_cast<jlongArray>(values);
}
void SetRegion(JNIEnv*, jlongArray, jsize start, jsize count, const jlong* source) {
  assert(start == 0 && count == 3);
  if (failure == Failure::Region) { pending = true; return; }
  values[0] = source[0]; values[1] = source[1]; values[2] = source[2];
}

int32_t HostSpawn(void*, const darwin_art_service_spawn_request_t* request,
                 darwin_art_service_spawn_result_t* result) {
  assert(request != nullptr && result != nullptr);
  assert(request->struct_size == sizeof(*request));
  assert(request->abi_version == DARWIN_ART_ABI_VERSION);
  assert(std::strcmp(request->component, "service") == 0);
  if (failure == Failure::Spawn) return -1;
  fd = open("/dev/null", O_RDONLY | O_CLOEXEC);
  assert(fd >= 0);
  fixture_lifetime = darwin_art::binder::WireChannelLifetime::Create();
  assert(fixture_lifetime && fixture_lifetime->Live());
  result->host_pid = 4242;
  result->control_fd = fd;
  return 0;
}

int32_t HostRelease(void*, int32_t pid) {
  assert(pid == 4242);
  ++releases;
  return 0;
}
}
namespace darwin_art {
std::shared_ptr<binder::WireChannelLifetime>
CaptureEstablishedRemoteBinderChannelLifetime(jint control) {
  assert(control == fd);
  assert(fixture_lifetime && fixture_lifetime->Live());
  capture_called = true;
  captured_generation = fixture_lifetime->Generation();
  return fixture_lifetime;
}
bool SendServiceBindIntent(JNIEnv*, jint, jobject) { return failure != Failure::Send; }
bool StartRemoteBinderDispatcher(JNIEnv*, jint control) {
  assert(control == fd);
  assert(capture_called);
  dispatcher_called = true;
  if (failure == Failure::Dispatcher) return false;
  if (failure == Failure::Revoked) {
    assert(fixture_lifetime && fixture_lifetime->Seal());
  }
  return true;
}
void CloseRemoteBinderChannel(JNIEnv*, jint control) {
  assert(control == fd);
  ++channels;
  if (fixture_lifetime) {
    fixture_lifetime->Seal();
    fixture_lifetime.reset();
  }
}
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
  table.NewLongArray = NewArray;
  table.SetLongArrayRegion = SetRegion;
  JNIEnv env{&table};
  assert(darwin_art::framework::os::RegisterServiceProcessTransport(&env, reinterpret_cast<jclass>(1)));
  assert(spawn && release);
  darwin_art_host_services_t services{
      sizeof(services), DARWIN_ART_ABI_VERSION, nullptr, HostSpawn, HostRelease};
  assert(!darwin_art::process::InstallHostServices(&services));
  services.context = &services;
  assert(darwin_art::process::InstallHostServices(&services));
  darwin_art::binder::WireChannelLifetimeTestPeer::SetNextGeneration(
      UINT64_C(1) << 63);
  for (auto current : {Failure::String, Failure::Spawn, Failure::Send,
                       Failure::Dispatcher, Failure::Revoked, Failure::Array,
                       Failure::Region, Failure::None}) {
    failure = current;
    pending = false;
    releases = channels = 0;
    fd = -1;
    capture_called = dispatcher_called = false;
    captured_generation = 0;
    fixture_lifetime.reset();
    auto text = reinterpret_cast<jstring>(const_cast<char*>("service"));
    auto result = spawn(&env, nullptr, text, text, text, JNI_FALSE, reinterpret_cast<jobject>(1));
    if (failure == Failure::None) {
      assert(result && !pending && releases == 0 && channels == 0);
      assert(capture_called && dispatcher_called);
      assert(values[0] == 4242 && values[1] == fd &&
             values[2] == std::bit_cast<jlong>(captured_generation) &&
             values[2] < 0 && fcntl(fd, F_GETFD) >= 0);
      assert(release(&env, nullptr, static_cast<jint>(values[0]),
                     static_cast<jint>(values[1])) == 0);
      assert(releases == 1 && channels == 1 && !fixture_lifetime);
    } else {
      assert(!result);
      const bool before_spawn = failure == Failure::Spawn || failure == Failure::String;
      assert(releases == (before_spawn ? 0 : 1));
      assert(channels == (before_spawn ? 0 : 1));
      const bool should_capture = failure != Failure::Spawn &&
                                  failure != Failure::String &&
                                  failure != Failure::Send;
      assert(capture_called == should_capture);
      assert(dispatcher_called == (failure == Failure::Dispatcher ||
                                   failure == Failure::Revoked ||
                                   failure == Failure::Array ||
                                   failure == Failure::Region));
      assert(pending == (failure == Failure::String || failure == Failure::Array || failure == Failure::Region));
      assert(!fixture_lifetime);
    }
    if (fd >= 0) { errno = 0; assert(fcntl(fd, F_GETFD) == -1 && errno == EBADF); }
  }
  darwin_art::process::ClearHostServices();
  std::puts("service process transport: failure rollback, pending exceptions, successful ownership transfer PASS");
}
