#include "../../compat/media/image_reader_jni.h"
#include "../../compat/darwin_angle_egl.h"
#include <android/hardware_buffer.h>
#include <cassert>
#include <cstdarg>
#include <cstdio>
#include <cstring>
#include <string>
#include <unordered_map>

struct AHardwareBuffer {};
namespace {
struct Object { jlong fields[8]{}; } reader, image;
std::unordered_map<std::string, void*> methods;
JNIEnv env{};
JavaVM vm{};
bool exception = false;
bool verify_release_order = false;
int fail_write = 0, writes = 0, returns = 0, buffer_refs = 0;
struct Window {
  int refs = 1;
  void (*callback)(void*, AHardwareBuffer*, int32_t, int, int32_t) = nullptr;
  void* context = nullptr;
  void (*destroy)(void*) = nullptr;
} *window = nullptr;
AHardwareBuffer buffer;
jfieldID Field(JNIEnv*, jclass, const char* name, const char*) {
  const char* names[] = {"", "mNativeContext", "mNativeBuffer", "mTimestamp",
                        "mDataSpace", "mTransform", "mScalingMode"};
  for (uintptr_t i = 1; i < 7; ++i)
    if (std::strcmp(name, names[i]) == 0) return reinterpret_cast<jfieldID>(i);
  assert(false); return nullptr;
}
void Set(JNIEnv*, jobject object, jfieldID field, jlong value) {
  assert(!exception);
  if (fail_write != 0 && ++writes == fail_write) { exception = true; return; }
  reinterpret_cast<Object*>(object)->fields[reinterpret_cast<uintptr_t>(field)] = value;
}
void SetInt(JNIEnv* e, jobject object, jfieldID field, jint value) {
  Set(e, object, field, value);
}
jlong Get(JNIEnv* e, jobject object, jfieldID field) {
  assert(e != &env || !exception);
  return reinterpret_cast<Object*>(object)->fields[reinterpret_cast<uintptr_t>(field)];
}
template<class T> T Method(const char* name) {
  assert(methods.count(name));
  return reinterpret_cast<T>(methods.at(name));
}
void Enqueue() {
  assert(window && window->callback);
  window->callback(window->context, &buffer, 1, -1, 42);
}
}

extern "C" void* darwin_art_android_ANativeWindow_create(int32_t, int32_t, int32_t) {
  assert(!window); window = new Window; return window;
}
extern "C" void darwin_art_android_ANativeWindow_acquire(void* value) {
  assert(value == window); ++window->refs;
}
extern "C" void darwin_art_android_ANativeWindow_release(void* value) {
  assert(value == window && window->refs > 0);
  if (--window->refs == 0) { delete window; window = nullptr; }
}
extern "C" bool darwin_art_android_ANativeWindow_set_owned_queue_callback(
    void* value, void (*callback)(void*, AHardwareBuffer*, int32_t, int, int32_t),
    void* context, void (*destroy)(void*)) {
  assert(value == window);
  auto old = window->destroy; auto old_context = window->context;
  window->callback = callback; window->context = context; window->destroy = destroy;
  if (old) old(old_context);
  return true;
}
extern "C" void darwin_art_android_ANativeWindow_release_consumer_slot(
    void* value, int32_t slot, int fence) {
  assert(value == window && slot == 1 && fence == -1);
  if (verify_release_order) {
    // A separate JNI environment models a competing acquisition at the exact
    // return boundary, without clearing the original thread's exception.
    JNIEnv competing = env;
    Object another_image;
    assert(Method<jint(*)(JNIEnv*, jobject, jobject)>("nativeImageSetup")(
        &competing, reinterpret_cast<jobject>(&reader),
        reinterpret_cast<jobject>(&another_image)) == 2);
  }
  ++returns;
}
extern "C" int darwin_art_bionic_socket_broker_close(int) { assert(false); return -1; }
extern "C" void AHardwareBuffer_acquire(AHardwareBuffer* b) {
  assert(b == &buffer); ++buffer_refs;
}
extern "C" void AHardwareBuffer_release(AHardwareBuffer* b) {
  assert(b == &buffer && buffer_refs > 0); --buffer_refs;
}
extern "C" void AHardwareBuffer_describe(const AHardwareBuffer*, AHardwareBuffer_Desc* d) {
  *d = {}; d->width = 16; d->height = 16; d->format = 1;
}
extern "C" jobject AHardwareBuffer_toHardwareBuffer(JNIEnv*, AHardwareBuffer*) { return nullptr; }

int main() {
  JNINativeInterface functions{};
  functions.FindClass = [](JNIEnv*, const char*) { return reinterpret_cast<jclass>(1); };
  functions.RegisterNatives = [](JNIEnv*, jclass, const JNINativeMethod* list, jint count) {
    for (int i = 0; i < count; ++i) methods[list[i].name] = list[i].fnPtr;
    return JNI_OK;
  };
  functions.DeleteLocalRef = [](JNIEnv*, jobject) {};
  functions.NewGlobalRef = [](JNIEnv*, jobject value) { return value; };
  functions.DeleteGlobalRef = [](JNIEnv*, jobject) {};
  functions.GetFieldID = Field;
  functions.GetStaticMethodID = [](JNIEnv*, jclass, const char*, const char*) {
    return reinterpret_cast<jmethodID>(1);
  };
  functions.CallStaticVoidMethodV = [](JNIEnv*, jclass, jmethodID, va_list) {};
  functions.GetJavaVM = [](JNIEnv*, JavaVM** out) { *out = &vm; return JNI_OK; };
  functions.GetLongField = Get; functions.SetLongField = Set; functions.SetIntField = SetInt;
  functions.ExceptionCheck = [](JNIEnv* e) -> jboolean { return e == &env && exception; };
  JNIInvokeInterface invocation{};
  invocation.GetEnv = [](JavaVM*, void** out, jint) { *out = &env; return JNI_OK; };
  env.functions = &functions; vm.functions = &invocation;
  assert(darwin_art::media::RegisterImageReaderNatives(&env));
  Method<void(*)(JNIEnv*, jclass)>("nativeClassInit")(&env, reinterpret_cast<jclass>(1));
  auto object = reinterpret_cast<jobject>(&reader);
  auto image_object = reinterpret_cast<jobject>(&image);
  Method<void(*)(JNIEnv*, jobject, jobject, jint, jint, jint, jlong, jint, jint)>(
      "nativeInit")(&env, object, object, 16, 16, 1, 0, 1, 0);
  assert(window && window->refs == 1);
  auto setup = Method<jint(*)(JNIEnv*, jobject, jobject)>("nativeImageSetup");
  auto release = Method<void(*)(JNIEnv*, jobject, jobject)>("nativeReleaseImage");
  for (int position = 1; position <= 5; ++position) {
    Enqueue(); fail_write = position; writes = 0;
    int previous = returns;
    verify_release_order = true;
    assert(setup(&env, object, image_object) == 1);
    verify_release_order = false;
    assert(exception && writes == position && image.fields[2] == 0);
    assert(returns == previous + 1 && buffer_refs == 0 && window->refs == 1);
    exception = false; fail_write = 0;
    // Failure must also return the maxImages accounting slot, so retry works.
    for (int field = 3; field <= 6; ++field) image.fields[field] = -10;
    Enqueue();
    assert(setup(&env, object, image_object) == 0 && image.fields[2] != 0);
    assert(image.fields[4] == 42 && buffer_refs == 1 && window->refs == 2);
    assert(image.fields[3] > 0 && image.fields[5] == 0 && image.fields[6] == 0);
    release(&env, object, image_object);
    assert(image.fields[2] == 0 && buffer_refs == 0 && window->refs == 1);
  }
  Enqueue();
  assert(setup(&env, object, image_object) == 0);
  Method<void(*)(JNIEnv*, jobject)>("nativeClose")(&env, object);
  assert(reader.fields[1] == 0 && window && buffer_refs == 1);
  release(&env, object, image_object);
  assert(window == nullptr && buffer_refs == 0 && returns == 11);
  std::puts("image-reader-jni: PASS registration publication-failure retry retained-image-close");
}
