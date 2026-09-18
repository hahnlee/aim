#include "../../compat/darwin_media_codec.h"

#include <cassert>
#include <cstdarg>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <string>
#include <unordered_map>
#include <vector>

namespace {

struct FakeObject : _jobject {
  jlong native_context = 0;
  const char* text = nullptr;
  int value = 0;
};

struct FakeArray : _jobject {
  std::vector<jobject> values;
};

struct FakeWindow {
  int references = 1;
  int release_calls = 0;
  bool managed = true;
};

std::unordered_map<std::string, void*> methods;
std::string current_class;
bool exception_pending = false;
int window_create_calls = 0;
int window_release_calls = 0;

std::string MethodKey(const char* name) { return current_class + "::" + name; }

jclass FindClass(JNIEnv*, const char* name) {
  current_class = name == nullptr ? "" : name;
  return reinterpret_cast<jclass>(1);
}

jint RegisterNatives(JNIEnv*, jclass, const JNINativeMethod* entries,
                     jint count) {
  for (jint index = 0; index < count; ++index) {
    methods[MethodKey(entries[index].name)] = entries[index].fnPtr;
  }
  return JNI_OK;
}

jclass GetObjectClass(JNIEnv*, jobject object) {
  return object == nullptr ? reinterpret_cast<jclass>(1)
                           : reinterpret_cast<jclass>(object);
}

jfieldID GetFieldID(JNIEnv*, jclass, const char* name, const char* signature) {
  assert(std::strcmp(name, "mNativeContext") == 0);
  assert(std::strcmp(signature, "J") == 0);
  return reinterpret_cast<jfieldID>(3);
}

jlong GetLongField(JNIEnv*, jobject object, jfieldID) {
  return static_cast<FakeObject*>(object)->native_context;
}

void SetLongField(JNIEnv*, jobject object, jfieldID, jlong value) {
  static_cast<FakeObject*>(object)->native_context = value;
}

jmethodID GetMethodID(JNIEnv*, jclass, const char* name, const char*) {
  (void)name;
  return reinterpret_cast<jmethodID>(1);
}

jmethodID GetStaticMethodID(JNIEnv*, jclass, const char* name, const char*) {
  (void)name;
  return reinterpret_cast<jmethodID>(2);
}

const char* GetStringUTFChars(JNIEnv*, jstring object, jboolean*) {
  return reinterpret_cast<FakeObject*>(object)->text;
}

void ReleaseStringUTFChars(JNIEnv*, jstring, const char*) {}

jsize GetArrayLength(JNIEnv*, jarray object) {
  return static_cast<jsize>(
      reinterpret_cast<FakeArray*>(object)->values.size());
}

jobject GetObjectArrayElement(JNIEnv*, jobjectArray object, jsize index) {
  return reinterpret_cast<FakeArray*>(object)->values.at(
      static_cast<size_t>(index));
}

jboolean IsInstanceOf(JNIEnv*, jobject, jclass) { return JNI_FALSE; }

jint CallIntMethod(JNIEnv*, jobject object, jmethodID, ...) {
  return reinterpret_cast<FakeObject*>(object)->value;
}

jint CallIntMethodV(JNIEnv*, jobject object, jmethodID, va_list) {
  return reinterpret_cast<FakeObject*>(object)->value;
}

jboolean ExceptionCheck(JNIEnv*) { return exception_pending ? JNI_TRUE : JNI_FALSE; }
void ExceptionDescribe(JNIEnv*) {}
void ExceptionClear(JNIEnv*) { exception_pending = false; }
jint ThrowNew(JNIEnv*, jclass, const char*) {
  exception_pending = true;
  return JNI_OK;
}
void DeleteLocalRef(JNIEnv*, jobject) {}

template <typename Function>
Function Native(const char* class_name, const char* method_name) {
  const auto found = methods.find(std::string(class_name) + "::" + method_name);
  assert(found != methods.end());
  return reinterpret_cast<Function>(found->second);
}

}  // namespace

extern "C" void* darwin_art_android_ANativeWindow_create(int32_t, int32_t,
                                                           int32_t) {
  ++window_create_calls;
  return new FakeWindow();
}

extern "C" void* darwin_art_android_ANativeWindow_fromSurface(
    void*, void* surface) {
  auto* window = reinterpret_cast<FakeWindow*>(
      static_cast<uintptr_t>(static_cast<FakeObject*>(surface)->native_context));
  assert(window != nullptr && window->managed);
  ++window->references;
  return window;
}

extern "C" void darwin_art_android_ANativeWindow_acquire(void* opaque) {
  auto* window = static_cast<FakeWindow*>(opaque);
  assert(window != nullptr && window->managed);
  ++window->references;
}

extern "C" void darwin_art_android_ANativeWindow_release(void* opaque) {
  auto* window = static_cast<FakeWindow*>(opaque);
  assert(window != nullptr && window->references > 0);
  ++window_release_calls;
  ++window->release_calls;
  if (--window->references == 0) window->managed = false;
}

extern "C" bool darwin_art_android_ANativeWindow_is_managed(void* opaque) {
  return opaque != nullptr && static_cast<FakeWindow*>(opaque)->managed;
}

extern "C" int32_t darwin_art_android_ANativeWindow_setBuffersGeometry(
    void*, int32_t, int32_t, int32_t) {
  return 0;
}
extern "C" int32_t darwin_art_android_ANativeWindow_lock(void*, void*, void*) {
  return -1;
}
extern "C" int32_t darwin_art_android_ANativeWindow_unlockAndPost(void*) {
  return 0;
}

int main() {
  JNINativeInterface_ functions{};
  functions.FindClass = &FindClass;
  functions.RegisterNatives = &RegisterNatives;
  functions.GetObjectClass = &GetObjectClass;
  functions.GetFieldID = &GetFieldID;
  functions.GetLongField = &GetLongField;
  functions.SetLongField = &SetLongField;
  functions.GetMethodID = &GetMethodID;
  functions.GetStaticMethodID = &GetStaticMethodID;
  functions.GetStringUTFChars = &GetStringUTFChars;
  functions.ReleaseStringUTFChars = &ReleaseStringUTFChars;
  functions.GetArrayLength = &GetArrayLength;
  functions.GetObjectArrayElement = &GetObjectArrayElement;
  functions.IsInstanceOf = &IsInstanceOf;
  functions.CallIntMethod = &CallIntMethod;
  functions.CallIntMethodV = &CallIntMethodV;
  functions.ExceptionCheck = &ExceptionCheck;
  functions.ExceptionDescribe = &ExceptionDescribe;
  functions.ExceptionClear = &ExceptionClear;
  functions.ThrowNew = &ThrowNew;
  functions.DeleteLocalRef = &DeleteLocalRef;
  JNIEnv env{&functions};

  assert(darwin_art::RegisterDarwinMediaCodecNatives(&env));
  auto setup = Native<void (*)(JNIEnv*, jobject, jstring, jboolean, jboolean,
                               jint, jint)>("android/media/MediaCodec",
                                             "native_setup");
  auto configure = Native<void (*)(JNIEnv*, jobject, jobjectArray, jobjectArray,
                                   jobject, jobject, jobject, jint)>(
      "android/media/MediaCodec", "native_configure");
  auto set_surface = Native<void (*)(JNIEnv*, jobject, jobject)>(
      "android/media/MediaCodec", "native_setSurface");
  auto release = Native<void (*)(JNIEnv*, jobject)>("android/media/MediaCodec",
                                                     "native_release");

  FakeObject codec;
  FakeObject mime{.text = "video/x-vnd.on2.vp9"};
  FakeObject first_surface;
  FakeObject second_surface;
  FakeWindow* first_window = static_cast<FakeWindow*>(
      darwin_art_android_ANativeWindow_create(32, 24, 1));
  FakeWindow* second_window = static_cast<FakeWindow*>(
      darwin_art_android_ANativeWindow_create(32, 24, 1));
  first_surface.native_context = reinterpret_cast<jlong>(first_window);
  second_surface.native_context = reinterpret_cast<jlong>(second_window);

  FakeObject key_mime{.text = "mime"};
  FakeObject key_width{.text = "width"};
  FakeObject key_height{.text = "height"};
  FakeObject value_mime{.text = "video/x-vnd.on2.vp9"};
  FakeObject value_width{.value = 32};
  FakeObject value_height{.value = 24};
  FakeArray keys;
  keys.values = {reinterpret_cast<jobject>(&key_mime),
                 reinterpret_cast<jobject>(&key_width),
                 reinterpret_cast<jobject>(&key_height)};
  FakeArray values;
  values.values = {reinterpret_cast<jobject>(&value_mime),
                   reinterpret_cast<jobject>(&value_width),
                   reinterpret_cast<jobject>(&value_height)};

  setup(&env, &codec, reinterpret_cast<jstring>(&mime), JNI_TRUE, JNI_FALSE, 0,
        0);
  assert(!exception_pending && codec.native_context != 0);
  configure(&env, &codec, reinterpret_cast<jobjectArray>(&keys),
            reinterpret_cast<jobjectArray>(&values), &first_surface, nullptr,
            nullptr, 0);
  assert(!exception_pending);

  // Surface release retires only the Java handle; the configured codec keeps
  // the native producer alive. Real native_setSurface then swaps ownership and
  // releases the old producer exactly once.
  darwin_art_android_ANativeWindow_release(first_window);
  assert(first_window->managed);
  set_surface(&env, &codec, &second_surface);
  assert(!exception_pending && !first_window->managed &&
         second_window->references == 2);
  darwin_art_android_ANativeWindow_release(second_window);
  assert(second_window->managed);
  release(&env, &codec);
  assert(codec.native_context == 0 && !second_window->managed);
  release(&env, &codec);
  assert(window_create_calls == 2 && window_release_calls == 4);
  assert(first_window->release_calls == 2 && second_window->release_calls == 2);
  delete first_window;
  delete second_window;

  std::puts("MediaCodec production surface lifetime: native_setSurface retention "
            "and release-once PASS");
  return 0;
}
