#include "window/texture_view_jni.h"

#include <android/native_window.h>

#include <cassert>
#include <cstdint>
#include <cstring>
#include <string>
#include <unordered_map>

struct ACanvas {};

namespace {

enum FieldTag : uintptr_t {
  kNativeWindow = 1,
  kLeft = 2,
  kTop = 3,
  kRight = 4,
  kBottom = 5,
};

struct Object : _jobject {
  jlong native_window = 0;
  int left = 0;
  int top = 0;
  int right = 0;
  int bottom = 0;
  void* producer = nullptr;
};

struct Window {
  int references = 1;  // SurfaceTexture's own producer reference.
  int acquire_calls = 0;
  int release_calls = 0;
  bool managed = true;
  int lock_status = 0;
  int unlock_calls = 0;
  int dataspace = 77;
  ANativeWindow_Buffer buffer{8, 6, 8, WINDOW_FORMAT_RGBA_8888, nullptr, {0}};
  bool locked = false;
  bool canvas_bind_ok = true;
  ARect clipped{};
  int clip_calls = 0;
};

struct Registration {
  std::string signature;
  void* function = nullptr;
};

std::unordered_map<std::string, Registration> methods;
const char* current_class = nullptr;
bool pending_exception = false;
bool fail_registration = false;
bool fail_field_lookup = false;
bool fail_canvas_bind = false;
bool fail_rect_write = false;
ACanvas canvas;
bool canvas_bound = false;
int32_t canvas_dataspace = 0;
ARect canvas_clip{};
int canvas_clip_calls = 0;

jclass FindClass(JNIEnv*, const char* name) {
  current_class = name;
  if (std::strcmp(name, "android/graphics/Rect") == 0 ||
      std::strcmp(name, "android/view/TextureView") == 0) {
    return reinterpret_cast<jclass>(1);
  }
  return nullptr;
}

jfieldID GetFieldID(JNIEnv*, jclass, const char* name, const char*) {
  if (fail_field_lookup) return nullptr;
  if (std::strcmp(current_class, "android/graphics/Rect") == 0) {
    if (std::strcmp(name, "left") == 0) return reinterpret_cast<jfieldID>(kLeft);
    if (std::strcmp(name, "top") == 0) return reinterpret_cast<jfieldID>(kTop);
    if (std::strcmp(name, "right") == 0) return reinterpret_cast<jfieldID>(kRight);
    if (std::strcmp(name, "bottom") == 0) return reinterpret_cast<jfieldID>(kBottom);
  }
  if (std::strcmp(current_class, "android/view/TextureView") == 0 &&
      std::strcmp(name, "mNativeWindow") == 0) {
    return reinterpret_cast<jfieldID>(kNativeWindow);
  }
  return nullptr;
}

jint RegisterNatives(JNIEnv*, jclass, const JNINativeMethod* entries,
                     jint count) {
  if (fail_registration) return JNI_ERR;
  assert(count == 4);
  for (jint i = 0; i < count; ++i) {
    methods.emplace(entries[i].name,
                    Registration{entries[i].signature, entries[i].fnPtr});
  }
  return JNI_OK;
}

void DeleteLocalRef(JNIEnv*, jobject) {}
jboolean ExceptionCheck(JNIEnv*) { return pending_exception ? JNI_TRUE : JNI_FALSE; }

jlong GetLongField(JNIEnv*, jobject object, jfieldID field) {
  assert(reinterpret_cast<uintptr_t>(field) == kNativeWindow);
  return static_cast<Object*>(object)->native_window;
}

void SetLongField(JNIEnv*, jobject object, jfieldID field, jlong value) {
  assert(reinterpret_cast<uintptr_t>(field) == kNativeWindow);
  static_cast<Object*>(object)->native_window = value;
}

jint GetIntField(JNIEnv*, jobject object, jfieldID field) {
  const auto tag = reinterpret_cast<uintptr_t>(field);
  const auto* value = static_cast<Object*>(object);
  if (tag == kLeft) return value->left;
  if (tag == kTop) return value->top;
  if (tag == kRight) return value->right;
  assert(tag == kBottom);
  return value->bottom;
}

void SetIntField(JNIEnv*, jobject object, jfieldID field, jint value) {
  if (fail_rect_write) {
    pending_exception = true;
    return;
  }
  const auto tag = reinterpret_cast<uintptr_t>(field);
  auto* target = static_cast<Object*>(object);
  if (tag == kLeft) target->left = value;
  else if (tag == kTop) target->top = value;
  else if (tag == kRight) target->right = value;
  else {
    assert(tag == kBottom);
    target->bottom = value;
  }
}

template <typename Function>
Function Native(const char* name, const char* signature) {
  const auto found = methods.find(name);
  assert(found != methods.end());
  assert(found->second.signature == signature);
  return reinterpret_cast<Function>(found->second.function);
}

}  // namespace

extern "C" jlong darwin_art_android_surface_texture_acquire_producer(
    JNIEnv*, jobject object) {
  auto* surface = static_cast<Object*>(object);
  auto* window = static_cast<Window*>(surface->producer);
  if (window == nullptr || !window->managed) return 0;
  ++window->references;
  ++window->acquire_calls;
  return reinterpret_cast<jlong>(window);
}

extern "C" bool darwin_art_android_ANativeWindow_is_managed(void* opaque) {
  return opaque != nullptr && static_cast<Window*>(opaque)->managed;
}

extern "C" bool darwin_art_android_ANativeWindow_release_if_managed(
    void* opaque) {
  auto* window = static_cast<Window*>(opaque);
  if (window == nullptr || !window->managed) return false;
  assert(window->references > 0);
  --window->references;
  ++window->release_calls;
  if (window->references == 0) window->managed = false;
  return true;
}

extern "C" int32_t darwin_art_android_ANativeWindow_lock(
    void* opaque, void* output, void* dirty) {
  auto* window = static_cast<Window*>(opaque);
  if (!window->managed || window->lock_status != 0 || window->locked) {
    return window->lock_status != 0 ? window->lock_status : -16;
  }
  window->locked = true;
  *static_cast<ANativeWindow_Buffer*>(output) = window->buffer;
  (void)dirty;
  return 0;
}

extern "C" int32_t darwin_art_android_ANativeWindow_unlockAndPost(void* opaque) {
  auto* window = static_cast<Window*>(opaque);
  assert(window->managed && window->locked);
  window->locked = false;
  ++window->unlock_calls;
  return 0;
}

extern "C" int32_t darwin_art_android_ANativeWindow_cancel_locked_buffer(
    void* opaque) {
  auto* window = static_cast<Window*>(opaque);
  assert(window->managed && window->locked);
  window->locked = false;
  return 0;
}

extern "C" int32_t ANativeWindow_getBuffersDataSpace(ANativeWindow* opaque) {
  return static_cast<Window*>(reinterpret_cast<void*>(opaque))->dataspace;
}

extern "C" ACanvas* ACanvas_getNativeHandleFromJava(JNIEnv*, jobject) {
  return &canvas;
}

extern "C" bool ACanvas_setBuffer(ACanvas*, const ANativeWindow_Buffer* buffer,
                                   int32_t dataspace) {
  canvas_bound = buffer != nullptr;
  canvas_dataspace = dataspace;
  if (buffer == nullptr) return true;
  if (fail_canvas_bind) {
    canvas_bound = false;
    return false;
  }
  return true;
}

extern "C" void ACanvas_clipRect(ACanvas*, const ARect* clip, bool) {
  canvas_clip = *clip;
  ++canvas_clip_calls;
}

int main() {
  JNINativeInterface functions{};
  functions.FindClass = &FindClass;
  functions.GetFieldID = &GetFieldID;
  functions.RegisterNatives = &RegisterNatives;
  functions.DeleteLocalRef = &DeleteLocalRef;
  functions.ExceptionCheck = &ExceptionCheck;
  functions.GetLongField = &GetLongField;
  functions.SetLongField = &SetLongField;
  functions.GetIntField = &GetIntField;
  functions.SetIntField = &SetIntField;
  JNIEnv env{&functions};

  assert(darwin_art::window::RegisterTextureViewNatives(&env));
  assert(methods.size() == 4);
  auto create = Native<void (*)(JNIEnv*, jobject, jobject)>(
      "nCreateNativeWindow", "(Landroid/graphics/SurfaceTexture;)V");
  auto destroy = Native<void (*)(JNIEnv*, jobject)>("nDestroyNativeWindow",
                                                    "()V");
  auto lock = Native<jboolean (*)(JNIEnv*, jobject, jlong, jobject, jobject)>(
      "nLockCanvas", "(JLandroid/graphics/Canvas;Landroid/graphics/Rect;)Z");
  auto unlock = Native<void (*)(JNIEnv*, jobject, jlong, jobject)>(
      "nUnlockCanvasAndPost", "(JLandroid/graphics/Canvas;)V");

  Window first;
  Window second;
  Object texture_view;
  Object first_surface;
  Object second_surface;
  Object canvas_object;
  first_surface.producer = &first;
  second_surface.producer = &second;

  create(&env, &texture_view, &first_surface);
  assert(texture_view.native_window == reinterpret_cast<jlong>(&first));
  assert(first.references == 2 && first.acquire_calls == 1);
  create(&env, &texture_view, &second_surface);
  assert(texture_view.native_window == reinterpret_cast<jlong>(&second));
  assert(first.references == 1 && first.release_calls == 1);
  assert(second.references == 2 && second.acquire_calls == 1);

  Object dirty;
  dirty.left = -3;
  dirty.top = 2;
  dirty.right = 99;
  dirty.bottom = 99;
  assert(lock(&env, &texture_view, texture_view.native_window, &canvas_object,
              &dirty) == JNI_TRUE);
  assert(dirty.left == 0 && dirty.top == 2 && dirty.right == 8 &&
         dirty.bottom == 6);
  assert(canvas_bound && canvas_dataspace == second.dataspace &&
         canvas_clip_calls == 1 && canvas_clip.left == 0 &&
         canvas_clip.top == 2 && canvas_clip.right == 8 &&
         canvas_clip.bottom == 6);
  unlock(&env, &texture_view, texture_view.native_window, &canvas_object);
  assert(!canvas_bound);
  assert(second.unlock_calls == 1 && !second.locked);

  second.lock_status = -12;
  assert(lock(&env, &texture_view, texture_view.native_window, &canvas_object,
              nullptr) == JNI_FALSE);
  second.lock_status = 0;
  fail_canvas_bind = true;
  assert(lock(&env, &texture_view, texture_view.native_window, &canvas_object,
              nullptr) == JNI_FALSE);
  assert(second.unlock_calls == 1 && !second.locked);
  fail_canvas_bind = false;
  fail_rect_write = true;
  dirty.left = -1;
  pending_exception = false;
  assert(lock(&env, &texture_view, texture_view.native_window, &canvas_object,
              &dirty) == JNI_FALSE);
  assert(pending_exception && !second.locked && !canvas_bound);
  pending_exception = false;
  fail_rect_write = false;
  destroy(&env, &texture_view);
  assert(texture_view.native_window == 0 && second.references == 1 &&
         second.release_calls == 1);
  destroy(&env, &texture_view);
  assert(second.release_calls == 1);

  Object invalid_surface;
  texture_view.native_window = 0;
  create(&env, &texture_view, &invalid_surface);
  assert(texture_view.native_window == 0);

  methods.clear();
  fail_registration = true;
  assert(!darwin_art::window::RegisterTextureViewNatives(&env));

  return 0;
}
