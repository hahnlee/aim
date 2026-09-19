#include "image_reader_jni.h"
#include "image_consumer_queue.h"
#include "../darwin_angle_egl.h"

#include <android/hardware_buffer.h>
#include <android/hardware_buffer_jni.h>
#include <algorithm>
#include <atomic>
#include <chrono>
#include <cstdio>
#include <iterator>
#include <memory>
#include <new>
#include <utility>

extern "C" int darwin_art_bionic_socket_broker_close(int);

namespace darwin_art::media {
namespace {
struct ImageReaderFields {
  jfieldID context = nullptr;
  jfieldID image_buffer = nullptr;
  jfieldID image_timestamp = nullptr;
  jfieldID image_dataspace = nullptr;
  jfieldID image_transform = nullptr;
  jfieldID image_scaling_mode = nullptr;
  jclass reader_class = nullptr;
  jmethodID post_event = nullptr;
};
ImageReaderFields g_image_reader_fields;

struct DarwinImageReader {
  explicit DarwinImageReader(uint32_t max_images) : queue(max_images) {}
  std::atomic<uint32_t> references{1};
  JavaVM* vm = nullptr;
  void* producer = nullptr;
  jobject weak_self = nullptr;
  darwin_art::media::ImageConsumerQueue queue;
};

struct DarwinSurfaceImage {
  darwin_art::media::OwnedConsumerBuffer lease;
  DarwinImageReader* reader = nullptr;
};

void ReleaseImageReader(DarwinImageReader* reader) {
  if (reader == nullptr ||
      reader->references.fetch_sub(1, std::memory_order_acq_rel) != 1) {
    return;
  }
  reader->queue.close();
  if (reader->weak_self != nullptr) {
    JNIEnv* env = nullptr;
    bool attached = false;
    if (reader->vm->GetEnv(reinterpret_cast<void**>(&env), JNI_VERSION_1_6) !=
        JNI_OK) {
      attached = reader->vm->AttachCurrentThread(&env, nullptr) == JNI_OK;
    }
    if (env != nullptr) env->DeleteGlobalRef(reader->weak_self);
    if (attached) reader->vm->DetachCurrentThread();
  }
  if (reader->producer != nullptr)
    darwin_art_android_ANativeWindow_release(reader->producer);
  delete reader;
}

void ReleaseImageReaderCallbackContext(void* context) {
  ReleaseImageReader(static_cast<DarwinImageReader*>(context));
}

void ImageReaderQueueBuffer(void* context, AHardwareBuffer* buffer,
                            int32_t slot, uint64_t, uint64_t, int fence,
                            int32_t dataspace) {
  auto* reader = static_cast<DarwinImageReader*>(context);
  if (reader == nullptr || buffer == nullptr) {
    if (fence >= 0) (void)darwin_art_bionic_socket_broker_close(fence);
    return;
  }
  AHardwareBuffer_acquire(buffer);
  darwin_art::media::ConsumerFrame incoming{
      .lease = {reader->producer, slot, fence, buffer},
      .dataspace = dataspace,
      .timestamp_ns = std::chrono::duration_cast<std::chrono::nanoseconds>(
          std::chrono::steady_clock::now().time_since_epoch()).count(),
  };
  bool notify = false;
  try {
    notify = reader->queue.enqueue(std::move(incoming));
  } catch (const std::bad_alloc&) {
    // Return the incoming slot through its owner; do not unwind a
    // C producer callback or leak its acquire fence on queue allocation failure.
    std::fprintf(stderr, "ART ImageReader: queue allocation failed\n");
    return;
  }
  if (!notify || reader->weak_self == nullptr ||
      g_image_reader_fields.reader_class == nullptr ||
      g_image_reader_fields.post_event == nullptr) {
    return;
  }
  JNIEnv* env = nullptr;
  bool attached = false;
  if (reader->vm->GetEnv(reinterpret_cast<void**>(&env), JNI_VERSION_1_6) !=
      JNI_OK) {
    attached = reader->vm->AttachCurrentThread(&env, nullptr) == JNI_OK;
  }
  if (env != nullptr) {
    env->CallStaticVoidMethod(g_image_reader_fields.reader_class,
                              g_image_reader_fields.post_event,
                              reader->weak_self);
    if (env->ExceptionCheck()) env->ExceptionClear();
  }
  if (attached) reader->vm->DetachCurrentThread();
}

DarwinImageReader* GetImageReader(JNIEnv* env, jobject object) {
  return g_image_reader_fields.context == nullptr
             ? nullptr
             : reinterpret_cast<DarwinImageReader*>(static_cast<uintptr_t>(
                   env->GetLongField(object, g_image_reader_fields.context)));
}

void ImageReaderNativeClassInit(JNIEnv* env, jclass reader_class) {
  g_image_reader_fields.context =
      env->GetFieldID(reader_class, "mNativeContext", "J");
  g_image_reader_fields.reader_class =
      static_cast<jclass>(env->NewGlobalRef(reader_class));
  g_image_reader_fields.post_event = env->GetStaticMethodID(
      reader_class, "postEventFromNative", "(Ljava/lang/Object;)V");
  jclass image_class =
      env->FindClass("android/media/ImageReader$SurfaceImage");
  if (image_class == nullptr) return;
  g_image_reader_fields.image_buffer =
      env->GetFieldID(image_class, "mNativeBuffer", "J");
  g_image_reader_fields.image_timestamp =
      env->GetFieldID(image_class, "mTimestamp", "J");
  g_image_reader_fields.image_dataspace =
      env->GetFieldID(image_class, "mDataSpace", "I");
  g_image_reader_fields.image_transform =
      env->GetFieldID(image_class, "mTransform", "I");
  g_image_reader_fields.image_scaling_mode =
      env->GetFieldID(image_class, "mScalingMode", "I");
  env->DeleteLocalRef(image_class);
}

void ImageReaderNativeInit(JNIEnv* env, jobject object, jobject weak_self,
                           jint width, jint height, jint max_images, jlong,
                           jint format, jint) {
  auto* reader = new (std::nothrow) DarwinImageReader(
      static_cast<uint32_t>(std::max(1, max_images)));
  if (reader == nullptr) return;
  env->GetJavaVM(&reader->vm);
  reader->weak_self = env->NewGlobalRef(weak_self);
  reader->producer =
      darwin_art_android_ANativeWindow_create(width, height, format);
  if (reader->producer == nullptr || reader->weak_self == nullptr) {
    ReleaseImageReader(reader);
    return;
  }
  reader->references.fetch_add(1, std::memory_order_relaxed);
  if (!darwin_art_android_ANativeWindow_set_owned_queue_callback(
          reader->producer, &ImageReaderQueueBuffer, reader,
          &ReleaseImageReaderCallbackContext)) {
    reader->references.fetch_sub(1, std::memory_order_relaxed);
    ReleaseImageReader(reader);
    return;
  }
  env->SetLongField(object, g_image_reader_fields.context,
                    reinterpret_cast<jlong>(reader));
}

void ImageReaderNativeClose(JNIEnv* env, jobject object) {
  DarwinImageReader* reader = GetImageReader(env, object);
  if (reader == nullptr) return;
  env->SetLongField(object, g_image_reader_fields.context, 0);
  reader->queue.close();
  darwin_art_android_ANativeWindow_set_owned_queue_callback(
      reader->producer, nullptr, nullptr, nullptr);
  ReleaseImageReader(reader);
}

jobject ImageReaderNativeGetSurface(JNIEnv* env, jobject object) {
  DarwinImageReader* reader = GetImageReader(env, object);
  if (reader == nullptr) return nullptr;
  jclass surface_class = env->FindClass("android/view/Surface");
  jmethodID constructor = surface_class == nullptr
                              ? nullptr
                              : env->GetMethodID(surface_class, "<init>", "()V");
  jobject surface = constructor == nullptr
                        ? nullptr
                        : env->NewObject(surface_class, constructor);
  jfieldID native_object = surface_class == nullptr
                               ? nullptr
                               : env->GetFieldID(surface_class, "mNativeObject", "J");
  if (surface != nullptr && native_object != nullptr) {
    darwin_art_android_ANativeWindow_acquire(reader->producer);
    env->SetLongField(surface, native_object,
                      reinterpret_cast<jlong>(reader->producer));
  }
  if (surface_class != nullptr) env->DeleteLocalRef(surface_class);
  return surface;
}

jint ImageReaderNativeImageSetup(JNIEnv* env, jobject object, jobject image) {
  DarwinImageReader* reader = GetImageReader(env, object);
  if (reader == nullptr) return 1;
  auto acquisition = reader->queue.acquireNext();
  if (acquisition.status != darwin_art::media::ImageAcquireStatus::Ok)
    return static_cast<jint>(acquisition.status);
  auto& pending = acquisition.frame;
  std::unique_ptr<DarwinSurfaceImage> native_image(
      new (std::nothrow) DarwinSurfaceImage{
          .lease = std::move(pending.lease), .reader = reader});
  if (native_image == nullptr) {
    pending.lease.reset();
    reader->queue.releaseAcquired();
    return 1;
  }
  // Publish metadata before the owning pointer. A pending JNI exception must
  // leave no native image attached to a Java object whose setup failed.
  env->SetLongField(image, g_image_reader_fields.image_timestamp,
                    pending.timestamp_ns);
  if (!env->ExceptionCheck())
    env->SetIntField(image, g_image_reader_fields.image_dataspace, pending.dataspace);
  if (!env->ExceptionCheck())
    env->SetIntField(image, g_image_reader_fields.image_transform, 0);
  if (!env->ExceptionCheck())
    env->SetIntField(image, g_image_reader_fields.image_scaling_mode, 0);
  if (!env->ExceptionCheck())
    env->SetLongField(image, g_image_reader_fields.image_buffer,
                      reinterpret_cast<jlong>(native_image.get()));
  if (env->ExceptionCheck()) {
    native_image.reset();
    reader->queue.releaseAcquired();
    return 1;  // Preserve the exception; the lease has returned its slot.
  }
  reader->references.fetch_add(1, std::memory_order_relaxed);
  (void)native_image.release();
  return 0;
}

DarwinSurfaceImage* GetSurfaceImage(JNIEnv* env, jobject image) {
  return g_image_reader_fields.image_buffer == nullptr
             ? nullptr
             : reinterpret_cast<DarwinSurfaceImage*>(static_cast<uintptr_t>(
                   env->GetLongField(image,
                                     g_image_reader_fields.image_buffer)));
}

void ImageReaderNativeReleaseImage(JNIEnv* env, jobject, jobject image) {
  DarwinSurfaceImage* native_image = GetSurfaceImage(env, image);
  if (native_image == nullptr) return;
  env->SetLongField(image, g_image_reader_fields.image_buffer, 0);
  native_image->lease.reset();
  native_image->reader->queue.releaseAcquired();
  ReleaseImageReader(native_image->reader);
  delete native_image;
}

void ImageReaderNativeDiscardFreeBuffers(JNIEnv*, jobject object) {
  // The producer's fixed three-slot pool is reclaimed as each queued consumer
  // slot is released. There is no separate gralloc cache to discard on Darwin.
  (void)object;
}

jint ImageReaderNativeDetachImage(JNIEnv*, jobject, jobject, jboolean) {
  // Detaching transfers GraphicBuffer ownership outside the reader. The Java
  // HardwareBuffer path used by Chromium does not detach; report unsupported
  // without corrupting the acquired slot's ownership.
  return -1;
}

jobjectArray ImageReaderNativeCreateImagePlanes(JNIEnv* env, jclass,
                                                 jint count, jobject, jint,
                                                 jint, jint, jint, jint,
                                                 jint) {
  jclass plane = env->FindClass("android/media/ImageReader$ImagePlane");
  jobjectArray result = plane == nullptr
                            ? nullptr
                            : env->NewObjectArray(std::max(0, count), plane,
                                                  nullptr);
  if (plane != nullptr) env->DeleteLocalRef(plane);
  return result;
}

void ImageReaderNativeUnlockGraphicBuffer(JNIEnv*, jclass, jobject) {}

jobjectArray SurfaceImageNativeCreatePlanes(JNIEnv* env, jobject, jint count,
                                             jint, jlong) {
  jclass plane = env->FindClass(
      "android/media/ImageReader$SurfaceImage$SurfacePlane");
  jobjectArray result = plane == nullptr
                            ? nullptr
                            : env->NewObjectArray(std::max(0, count), plane,
                                                  nullptr);
  if (plane != nullptr) env->DeleteLocalRef(plane);
  return result;
}

jint SurfaceImageNativeGetWidth(JNIEnv* env, jobject image) {
  DarwinSurfaceImage* native_image = GetSurfaceImage(env, image);
  if (native_image == nullptr) return 0;
  AHardwareBuffer_Desc desc{};
  AHardwareBuffer_describe(native_image->lease.buffer(), &desc);
  return static_cast<jint>(desc.width);
}
jint SurfaceImageNativeGetHeight(JNIEnv* env, jobject image) {
  DarwinSurfaceImage* native_image = GetSurfaceImage(env, image);
  if (native_image == nullptr) return 0;
  AHardwareBuffer_Desc desc{};
  AHardwareBuffer_describe(native_image->lease.buffer(), &desc);
  return static_cast<jint>(desc.height);
}
jint SurfaceImageNativeGetFormat(JNIEnv* env, jobject image, jint reader_format) {
  DarwinSurfaceImage* native_image = GetSurfaceImage(env, image);
  if (native_image == nullptr) return reader_format;
  AHardwareBuffer_Desc desc{};
  AHardwareBuffer_describe(native_image->lease.buffer(), &desc);
  return static_cast<jint>(desc.format);
}
jint SurfaceImageNativeGetFenceFd(JNIEnv* env, jobject image) {
  DarwinSurfaceImage* native_image = GetSurfaceImage(env, image);
  return native_image == nullptr ? -1 : native_image->lease.fence();
}

jobject SurfaceImageNativeGetHardwareBuffer(JNIEnv* env, jobject image) {
  DarwinSurfaceImage* native_image = GetSurfaceImage(env, image);
  if (native_image == nullptr || native_image->lease.buffer() == nullptr) return nullptr;
  return AHardwareBuffer_toHardwareBuffer(env, native_image->lease.buffer());
}

bool Register(JNIEnv* env, const char* name, JNINativeMethod* methods,
              jint count) {
  jclass klass = env->FindClass(name);
  if (klass == nullptr) return false;
  const bool result = env->RegisterNatives(klass, methods, count) == JNI_OK;
  env->DeleteLocalRef(klass);
  return result;
}
}  // namespace

bool RegisterImageReaderNatives(JNIEnv* env) {
  JNINativeMethod image_reader_methods[] = {
      {const_cast<char*>("nativeClassInit"), const_cast<char*>("()V"),
       reinterpret_cast<void*>(&ImageReaderNativeClassInit)},
      {const_cast<char*>("nativeInit"),
       const_cast<char*>("(Ljava/lang/Object;IIIJII)V"),
       reinterpret_cast<void*>(&ImageReaderNativeInit)},
      {const_cast<char*>("nativeClose"), const_cast<char*>("()V"),
       reinterpret_cast<void*>(&ImageReaderNativeClose)},
      {const_cast<char*>("nativeReleaseImage"),
       const_cast<char*>("(Landroid/media/Image;)V"),
       reinterpret_cast<void*>(&ImageReaderNativeReleaseImage)},
      {const_cast<char*>("nativeImageSetup"),
       const_cast<char*>("(Landroid/media/Image;)I"),
       reinterpret_cast<void*>(&ImageReaderNativeImageSetup)},
      {const_cast<char*>("nativeGetSurface"),
       const_cast<char*>("()Landroid/view/Surface;"),
       reinterpret_cast<void*>(&ImageReaderNativeGetSurface)},
      {const_cast<char*>("nativeDetachImage"),
       const_cast<char*>("(Landroid/media/Image;Z)I"),
       reinterpret_cast<void*>(&ImageReaderNativeDetachImage)},
      {const_cast<char*>("nativeCreateImagePlanes"),
       const_cast<char*>(
           "(ILandroid/graphics/GraphicBuffer;IIIIII)[Landroid/media/ImageReader$ImagePlane;"),
       reinterpret_cast<void*>(&ImageReaderNativeCreateImagePlanes)},
      {const_cast<char*>("nativeUnlockGraphicBuffer"),
       const_cast<char*>("(Landroid/graphics/GraphicBuffer;)V"),
       reinterpret_cast<void*>(&ImageReaderNativeUnlockGraphicBuffer)},
      {const_cast<char*>("nativeDiscardFreeBuffers"),
       const_cast<char*>("()V"),
       reinterpret_cast<void*>(&ImageReaderNativeDiscardFreeBuffers)},
  };
  if (!Register(env, "android/media/ImageReader", image_reader_methods,
                static_cast<jint>(std::size(image_reader_methods)))) {
    return false;
  }

  JNINativeMethod surface_image_methods[] = {
      {const_cast<char*>("nativeCreatePlanes"), const_cast<char*>("(IIJ)[Landroid/media/ImageReader$SurfaceImage$SurfacePlane;"),
       reinterpret_cast<void*>(&SurfaceImageNativeCreatePlanes)},
      {const_cast<char*>("nativeGetWidth"), const_cast<char*>("()I"),
       reinterpret_cast<void*>(&SurfaceImageNativeGetWidth)},
      {const_cast<char*>("nativeGetHeight"), const_cast<char*>("()I"),
       reinterpret_cast<void*>(&SurfaceImageNativeGetHeight)},
      {const_cast<char*>("nativeGetFormat"), const_cast<char*>("(I)I"),
       reinterpret_cast<void*>(&SurfaceImageNativeGetFormat)},
      {const_cast<char*>("nativeGetFenceFd"), const_cast<char*>("()I"),
       reinterpret_cast<void*>(&SurfaceImageNativeGetFenceFd)},
      {const_cast<char*>("nativeGetHardwareBuffer"),
       const_cast<char*>("()Landroid/hardware/HardwareBuffer;"),
       reinterpret_cast<void*>(&SurfaceImageNativeGetHardwareBuffer)},
  };
  if (!Register(env, "android/media/ImageReader$SurfaceImage",
                surface_image_methods,
                static_cast<jint>(std::size(surface_image_methods)))) {
    return false;
  }

  return true;
}
}  // namespace darwin_art::media
