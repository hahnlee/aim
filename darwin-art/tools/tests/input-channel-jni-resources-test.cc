#include "runtime/framework/input/input_channel_jni_resources.h"
#include <cassert>
#include <new>

static int strings = 0, refs = 0, closes = 0, throws = 0;
static bool pending = false;
static const char* GetUtf(JNIEnv*, jstring, jboolean*) { ++strings; return "channel"; }
static void ReleaseUtf(JNIEnv*, jstring, const char*) { assert(strings == 1); --strings; }
static void DeleteRef(JNIEnv*, jobject) { assert(strings == 0); ++refs; }
static jboolean Exception(JNIEnv*) { return pending ? JNI_TRUE : JNI_FALSE; }
static jclass FindClass(JNIEnv*, const char*) { return reinterpret_cast<jclass>(3); }
static jint ThrowNew(JNIEnv*, jclass, const char*) { ++throws; pending = true; return 0; }
extern "C" int darwin_art_bionic_socket_broker_close(int fd) {
  assert(fd == 77); ++closes; return 0;
}
int main() {
  JNINativeInterface table{};
  table.GetStringUTFChars = GetUtf;
  table.ReleaseStringUTFChars = ReleaseUtf;
  table.DeleteLocalRef = DeleteRef;
  table.ExceptionCheck = Exception;
  table.FindClass = FindClass;
  table.ThrowNew = ThrowNew;
  JNIEnv env{&table};
  using namespace darwin_art::input;
  InputChannelParcelData data{true, reinterpret_cast<jobject>(1),
                             reinterpret_cast<jstring>(2), 77};
  try {
    InputChannelParcelResources parcel(&env, data);
    InputChannelUtfChars utf(&env, data.name);
    assert(utf.Get() != nullptr);
    throw std::bad_alloc();
  } catch (const std::bad_alloc&) {}
  assert(strings == 0 && refs == 2 && closes == 1);
  assert(data.token == nullptr && data.name == nullptr && data.endpoint_fd == -1);
  { InputChannelParcelResources repeat(&env, data); }
  assert(refs == 2 && closes == 1);
  data.endpoint_fd = 77;
  { InputChannelParcelResources parcel(&env, data); data.endpoint_fd = -1; }
  assert(closes == 1);  // Transferred descriptor is no longer Parcel-owned.
  ThrowInputChannelOutOfMemory(&env);
  assert(throws == 1 && pending);
  ThrowInputChannelOutOfMemory(&env);
  assert(throws == 1);  // Preserve the preexisting Java exception.
}
