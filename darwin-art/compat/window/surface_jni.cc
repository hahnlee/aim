#include "surface_jni.h"
#include "blast_buffer_queue_jni.h"
#include "../darwin_android_native_window.h"
#include "../darwin_angle_egl.h"
#include "../darwin_android_surface_texture.h"
#include <android/graphics/canvas.h>

#include <algorithm>
#include <cstdint>
#include <cstdlib>
#include <iostream>
#include <iterator>
#include <unistd.h>

namespace {
// A detached host Surface is a process-local producer endpoint backed by the
// active IOSurface/CAMetalLayer bridge. Java owns the token lifetime; native
// video libraries receive the ordinary android.view.Surface object.
jboolean SurfaceNativeIsValid(JNIEnv*, jclass, jlong handle) {
  return handle != 0 ? JNI_TRUE : JNI_FALSE;
}

void ThrowSurfaceException(JNIEnv* env, const char* class_name) {
  if (env == nullptr || env->ExceptionCheck()) return;
  jclass exception_class = env->FindClass(class_name);
  if (exception_class == nullptr) return;
  env->ThrowNew(exception_class, nullptr);
  env->DeleteLocalRef(exception_class);
}

jlong SurfaceNativeLockCanvas(JNIEnv* env, jclass, jlong handle,
                              jobject canvas_object, jobject dirty_object) {
  auto* window = reinterpret_cast<void*>(static_cast<uintptr_t>(handle));
  const bool managed = window != nullptr &&
      darwin_art_android_ANativeWindow_is_managed(window);
  if (window == nullptr || canvas_object == nullptr || !managed) {
    if (std::getenv("DARWIN_ART_DEBUG_ANATIVEWINDOW") != nullptr) {
      std::cerr << "ART Android Surface: lock rejected handle=" << window
                << " canvas=" << canvas_object << " managed=" << managed
                << "\n";
    }
    ThrowSurfaceException(env, "java/lang/IllegalArgumentException");
    return 0;
  }

  ARect dirty{};
  ARect* dirty_pointer = nullptr;
  jclass rect_class = nullptr;
  jfieldID left_field = nullptr;
  jfieldID top_field = nullptr;
  jfieldID right_field = nullptr;
  jfieldID bottom_field = nullptr;
  if (dirty_object != nullptr) {
    rect_class = env->GetObjectClass(dirty_object);
    if (rect_class != nullptr) {
      left_field = env->GetFieldID(rect_class, "left", "I");
      top_field = env->GetFieldID(rect_class, "top", "I");
      right_field = env->GetFieldID(rect_class, "right", "I");
      bottom_field = env->GetFieldID(rect_class, "bottom", "I");
    }
    if (env->ExceptionCheck() || left_field == nullptr || top_field == nullptr ||
        right_field == nullptr || bottom_field == nullptr) {
      env->DeleteLocalRef(rect_class);
      return 0;
    }
    dirty.left = env->GetIntField(dirty_object, left_field);
    dirty.top = env->GetIntField(dirty_object, top_field);
    dirty.right = env->GetIntField(dirty_object, right_field);
    dirty.bottom = env->GetIntField(dirty_object, bottom_field);
    dirty_pointer = &dirty;
  }

  // WindowManager can construct the Surface with a logical PixelFormat value
  // such as OPAQUE (-1). It is not a renderable buffer format. AOSP's Surface
  // JNI performs this same fallback before locking a software Canvas.
  const int32_t window_format =
      darwin_art_android_ANativeWindow_getFormat(window);
  if (!ACanvas_isSupportedPixelFormat(window_format) &&
      darwin_art_android_ANativeWindow_setBuffersGeometry(window, 0, 0, 1) !=
          0) {
    env->DeleteLocalRef(rect_class);
    ThrowSurfaceException(env, "java/lang/IllegalArgumentException");
    return 0;
  }

  ANativeWindow_Buffer buffer{};
  const int32_t lock_status =
      darwin_art_android_ANativeWindow_lock(window, &buffer, dirty_pointer);
  if (lock_status != 0) {
    if (std::getenv("DARWIN_ART_DEBUG_ANATIVEWINDOW") != nullptr) {
      std::cerr << "ART Android Surface: ANativeWindow_lock failed handle="
                << window << " status=" << lock_status << "\n";
    }
    env->DeleteLocalRef(rect_class);
    ThrowSurfaceException(env, lock_status == -12
                                   ? "android/view/Surface$OutOfResourcesException"
                                   : "java/lang/IllegalArgumentException");
    return 0;
  }

  ACanvas* canvas = ACanvas_getNativeHandleFromJava(env, canvas_object);
  if (canvas == nullptr || !ACanvas_setBuffer(canvas, &buffer, 0)) {
    if (std::getenv("DARWIN_ART_DEBUG_ANATIVEWINDOW") != nullptr) {
      std::cerr << "ART Android Surface: Canvas buffer bind failed handle="
                << window << " canvas=" << canvas << "\n";
    }
    (void)darwin_art_android_ANativeWindow_unlockAndPost(window);
    env->DeleteLocalRef(rect_class);
    ThrowSurfaceException(env, "java/lang/IllegalArgumentException");
    return 0;
  }

  if (dirty_pointer != nullptr) {
    dirty.left = std::clamp(dirty.left, 0, buffer.width);
    dirty.top = std::clamp(dirty.top, 0, buffer.height);
    dirty.right = std::clamp(dirty.right, dirty.left, buffer.width);
    dirty.bottom = std::clamp(dirty.bottom, dirty.top, buffer.height);
    ACanvas_clipRect(canvas, &dirty, false);
    env->SetIntField(dirty_object, left_field, dirty.left);
    env->SetIntField(dirty_object, top_field, dirty.top);
    env->SetIntField(dirty_object, right_field, dirty.right);
    env->SetIntField(dirty_object, bottom_field, dirty.bottom);
  }
  env->DeleteLocalRef(rect_class);

  // Surface.java stores this independent locked reference in mLockedObject.
  // It remains valid if mNativeObject is replaced before unlock and is
  // released by Surface.java's finally block after nativeUnlockCanvasAndPost.
  darwin_art_android_ANativeWindow_acquire(window);
  return handle;
}

void SurfaceNativeUnlockCanvasAndPost(JNIEnv* env, jclass, jlong handle,
                                      jobject canvas_object) {
  auto* window = reinterpret_cast<void*>(static_cast<uintptr_t>(handle));
  if (window == nullptr || canvas_object == nullptr ||
      !darwin_art_android_ANativeWindow_is_managed(window)) {
    ThrowSurfaceException(env, "java/lang/IllegalArgumentException");
    return;
  }
  ACanvas* canvas = ACanvas_getNativeHandleFromJava(env, canvas_object);
  if (canvas == nullptr) {
    ThrowSurfaceException(env, "java/lang/IllegalArgumentException");
    return;
  }
  // Detach before queueing: the Java Canvas must not retain pixels after the
  // producer transfers this buffer to the compositor.
  (void)ACanvas_setBuffer(canvas, nullptr, 0);
  if (darwin_art_android_ANativeWindow_unlockAndPost(window) != 0) {
    ThrowSurfaceException(env, "java/lang/IllegalArgumentException");
  }
}

void SurfaceNativeRelease(JNIEnv*, jclass, jlong handle) {
  (void)darwin_art_android_ANativeWindow_release_if_managed(
      reinterpret_cast<void*>(static_cast<uintptr_t>(handle)));
}
void SurfaceNativeDestroy(JNIEnv*, jclass, jlong) {
  // Surface.destroy() is a producer disconnect, not a strong-reference
  // release. Surface.java immediately calls release() afterward, which owns
  // the single nativeRelease for this Java Surface. Mapping both natives to
  // SurfaceNativeRelease consumed BLASTBufferQueue's producer reference and
  // left nativeDestroy() locking a freed ANativeWindow during compositor
  // detach. The Darwin queue currently has no separate connected-API state,
  // so disconnect is intentionally a no-op while reference teardown remains
  // exclusively in SurfaceNativeRelease.
}
jlong SurfaceNativeCreateFromSurfaceTexture(JNIEnv* env, jclass,
                                            jobject surface_texture) {
  return darwin_art_android_surface_texture_acquire_producer(env,
                                                             surface_texture);
}
jint SurfaceNativeGetWidth(JNIEnv*, jclass, jlong handle) {
  return handle == 0
             ? 0
             : darwin_art_android_ANativeWindow_getWidth(
                   reinterpret_cast<void*>(static_cast<uintptr_t>(handle)));
}
jint SurfaceNativeGetHeight(JNIEnv*, jclass, jlong handle) {
  return handle == 0
             ? 0
             : darwin_art_android_ANativeWindow_getHeight(
                   reinterpret_cast<void*>(static_cast<uintptr_t>(handle)));
}
jlong SurfaceNativeGetNextFrameNumber(JNIEnv*, jclass, jlong handle) {
  return static_cast<jlong>(darwin_art_android_ANativeWindow_next_frame_number(
      reinterpret_cast<void*>(static_cast<std::uintptr_t>(handle))));
}
jboolean SurfaceNativeFalse(JNIEnv*, jclass, jlong) { return JNI_FALSE; }
void SurfaceNativeAllocateBuffers(JNIEnv*, jclass, jlong) {}
jint SurfaceNativeStatus(JNIEnv*, jclass, jlong, jint) { return 0; }
jint SurfaceNativeForceDisconnect(JNIEnv*, jclass, jlong) { return 0; }
jint SurfaceNativeSetBoolean(JNIEnv*, jclass, jlong, jboolean) { return 0; }
jint SurfaceNativeSetFrameRate(JNIEnv*, jclass, jlong, jfloat, jint, jint) {
  return 0;
}
jlong SurfaceNativeGetFromBlastBufferQueue(JNIEnv*, jclass, jlong old_surface,
                                           jlong blast_queue) {
  // BLAST owns the producer identity and returns one retained ANativeWindow
  // reference. Surface never reaches into the queue's private state.
  void* window =
      darwin_art_android_blast_buffer_queue_acquire_native_window(blast_queue);
  if (std::getenv("DARWIN_ART_DEBUG_ANATIVEWINDOW") != nullptr) {
    std::cerr << "ART Android Surface: from BLAST queue=" << blast_queue
              << " window=" << window
              << " old=" << reinterpret_cast<void*>(old_surface) << "\n";
  }
  return window == nullptr ? old_surface : reinterpret_cast<jlong>(window);
}
constexpr uint64_t kDarwinSurfaceParcelMagic = 0x4441534600000000ull;

jlong SurfaceNativeReadFromParcel(JNIEnv* env, jclass, jlong old_handle,
                                  jobject parcel) {
  if (env == nullptr || parcel == nullptr) return 0;
  jclass parcel_class = env->GetObjectClass(parcel);
  jmethodID read_long = parcel_class == nullptr
                            ? nullptr
                            : env->GetMethodID(parcel_class, "readLong", "()J");
  const jlong token = read_long == nullptr
                          ? 0
                          : env->CallLongMethod(parcel, read_long);
  if (env->ExceptionCheck() ||
      (static_cast<uint64_t>(token) & 0xffffffff00000000ull) !=
          kDarwinSurfaceParcelMagic) {
    env->DeleteLocalRef(parcel_class);
    return 0;
  }
  jmethodID read_int = env->GetMethodID(parcel_class, "readInt", "()I");
  const jint width =
      read_int == nullptr ? 0 : env->CallIntMethod(parcel, read_int);
  const jint height =
      read_int == nullptr ? 0 : env->CallIntMethod(parcel, read_int);
  const jint owner_process_id =
      read_int == nullptr ? 0 : env->CallIntMethod(parcel, read_int);
  const jint layer_id =
      read_int == nullptr ? 0 : env->CallIntMethod(parcel, read_int);
  const jint format =
      read_int == nullptr ? 1 : env->CallIntMethod(parcel, read_int);
  env->DeleteLocalRef(parcel_class);
  if (owner_process_id > 0 && layer_id > 0 && !env->ExceptionCheck()) {
    darwin_art_android_ANativeWindow_register_imported_surface_identity(
        token, static_cast<uint32_t>(owner_process_id),
        static_cast<uint32_t>(layer_id), width, height, format);
  }
  if (std::getenv("DARWIN_ART_DEBUG_ANATIVEWINDOW") != nullptr) {
    std::cerr << "ART Android Surface parcel: read pid=" << getpid()
              << " token=0x" << std::hex << static_cast<uint64_t>(token)
              << std::dec << " size=" << width << "x" << height << "\n";
  }
  static_cast<void>(old_handle);
  // This is a process-independent surface identity, not a native pointer.
  // The GPU process resolves the matching IOSurface through the host surface
  // broker before ANGLE creates its EGL window surface.
  return token;
}

void SurfaceNativeWriteToParcel(JNIEnv* env, jclass, jlong handle,
                                jobject parcel) {
  if (env == nullptr || parcel == nullptr) return;
  jclass parcel_class = env->GetObjectClass(parcel);
  jmethodID write_long =
      parcel_class == nullptr
          ? nullptr
          : env->GetMethodID(parcel_class, "writeLong", "(J)V");
  if (write_long != nullptr) {
    const uint64_t identity =
        kDarwinSurfaceParcelMagic |
        (handle == 0 ? 0ull : static_cast<uint64_t>(handle) & 0xffffffffull);
    env->CallVoidMethod(parcel, write_long, static_cast<jlong>(identity));
    jmethodID write_int =
        env->GetMethodID(parcel_class, "writeInt", "(I)V");
    if (write_int != nullptr) {
      void* window = reinterpret_cast<void*>(static_cast<uintptr_t>(handle));
      const jint width = darwin_art_android_ANativeWindow_getWidth(window);
      const jint height = darwin_art_android_ANativeWindow_getHeight(window);
      const jint format = darwin_art_android_ANativeWindow_getFormat(window);
      env->CallVoidMethod(parcel, write_int, width);
      env->CallVoidMethod(parcel, write_int, height);
      uint32_t owner_process_id = 0;
      uint32_t layer_id = 0;
      (void)darwin_art_android_ANativeWindow_get_surface_control_identity(
          reinterpret_cast<void*>(static_cast<uintptr_t>(handle)),
          &owner_process_id, &layer_id);
      env->CallVoidMethod(parcel, write_int,
                          static_cast<jint>(owner_process_id));
      env->CallVoidMethod(parcel, write_int, static_cast<jint>(layer_id));
      env->CallVoidMethod(parcel, write_int, format);
      if (std::getenv("DARWIN_ART_DEBUG_ANATIVEWINDOW") != nullptr) {
        std::cerr << "ART Android Surface parcel: write pid=" << getpid()
                  << " token=0x" << std::hex << identity << std::dec
                  << " size=" << width << "x" << height
                  << " format=" << format
                  << " owner=" << owner_process_id
                  << " layer=" << layer_id << "\n";
      }
    }
  }
  env->DeleteLocalRef(parcel_class);
}
bool Register(JNIEnv* env, const char* class_name, JNINativeMethod* methods,
              jint method_count) {
  if (env == nullptr) return false;
  jclass klass = env->FindClass(class_name);
  if (klass == nullptr) return false;
  const bool registered =
      env->RegisterNatives(klass, methods, method_count) == JNI_OK;
  env->DeleteLocalRef(klass);
  return registered;
}
}  // namespace

namespace darwin_art::window {

bool RegisterSurfaceNatives(JNIEnv* env) {
  JNINativeMethod methods[] = {
      {const_cast<char*>("nativeCreateFromSurfaceTexture"),
       const_cast<char*>("(Landroid/graphics/SurfaceTexture;)J"),
       reinterpret_cast<void*>(&SurfaceNativeCreateFromSurfaceTexture)},
      {const_cast<char*>("nativeGetFromBlastBufferQueue"),
       const_cast<char*>("(JJ)J"),
       reinterpret_cast<void*>(&SurfaceNativeGetFromBlastBufferQueue)},
      {const_cast<char*>("nativeIsValid"), const_cast<char*>("(J)Z"),
       reinterpret_cast<void*>(&SurfaceNativeIsValid)},
      {const_cast<char*>("nativeLockCanvas"),
       const_cast<char*>(
           "(JLandroid/graphics/Canvas;Landroid/graphics/Rect;)J"),
       reinterpret_cast<void*>(&SurfaceNativeLockCanvas)},
      {const_cast<char*>("nativeUnlockCanvasAndPost"),
       const_cast<char*>("(JLandroid/graphics/Canvas;)V"),
       reinterpret_cast<void*>(&SurfaceNativeUnlockCanvasAndPost)},
      {const_cast<char*>("nativeRelease"), const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&SurfaceNativeRelease)},
      {const_cast<char*>("nativeDestroy"), const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&SurfaceNativeDestroy)},
      {const_cast<char*>("nativeGetWidth"), const_cast<char*>("(J)I"),
       reinterpret_cast<void*>(&SurfaceNativeGetWidth)},
      {const_cast<char*>("nativeGetHeight"), const_cast<char*>("(J)I"),
       reinterpret_cast<void*>(&SurfaceNativeGetHeight)},
      {const_cast<char*>("nativeGetNextFrameNumber"), const_cast<char*>("(J)J"),
       reinterpret_cast<void*>(&SurfaceNativeGetNextFrameNumber)},
      {const_cast<char*>("nativeIsConsumerRunningBehind"),
       const_cast<char*>("(J)Z"), reinterpret_cast<void*>(&SurfaceNativeFalse)},
      {const_cast<char*>("nativeAllocateBuffers"), const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&SurfaceNativeAllocateBuffers)},
      {const_cast<char*>("nativeSetScalingMode"),
       const_cast<char*>("(JI)I"),
       reinterpret_cast<void*>(&SurfaceNativeStatus)},
      {const_cast<char*>("nativeForceScopedDisconnect"),
       const_cast<char*>("(J)I"),
       reinterpret_cast<void*>(&SurfaceNativeForceDisconnect)},
      {const_cast<char*>("nativeSetSharedBufferModeEnabled"),
       const_cast<char*>("(JZ)I"),
       reinterpret_cast<void*>(&SurfaceNativeSetBoolean)},
      {const_cast<char*>("nativeSetAutoRefreshEnabled"),
       const_cast<char*>("(JZ)I"),
       reinterpret_cast<void*>(&SurfaceNativeSetBoolean)},
      {const_cast<char*>("nativeSetFrameRate"),
       const_cast<char*>("(JFII)I"),
       reinterpret_cast<void*>(&SurfaceNativeSetFrameRate)},
      {const_cast<char*>("nativeReadFromParcel"),
       const_cast<char*>("(JLandroid/os/Parcel;)J"),
       reinterpret_cast<void*>(&SurfaceNativeReadFromParcel)},
      {const_cast<char*>("nativeWriteToParcel"),
       const_cast<char*>("(JLandroid/os/Parcel;)V"),
       reinterpret_cast<void*>(&SurfaceNativeWriteToParcel)},
  };
  return Register(env, "android/view/Surface", methods,
                  static_cast<jint>(std::size(methods)));
}

}  // namespace darwin_art::window
