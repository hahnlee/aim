#include "hardware_buffer_jni.h"

#include <algorithm>
#include <cstdint>
#include <iterator>

namespace {

AHardwareBuffer* HandleToBuffer(jlong handle) {
  return reinterpret_cast<AHardwareBuffer*>(static_cast<uintptr_t>(handle));
}

AHardwareBuffer_Desc Describe(jlong handle) {
  AHardwareBuffer_Desc desc{};
  if (handle != 0) AHardwareBuffer_describe(HandleToBuffer(handle), &desc);
  return desc;
}

void ThrowUnsupported(JNIEnv* env, const char* message) {
  if (env == nullptr || env->ExceptionCheck()) return;
  jclass exception = env->FindClass("java/lang/UnsupportedOperationException");
  if (exception == nullptr) return;
  env->ThrowNew(exception, message);
  env->DeleteLocalRef(exception);
}

}  // namespace

extern "C" AHardwareBuffer* AHardwareBuffer_fromHardwareBuffer(
    JNIEnv* env, jobject hardware_buffer) {
  if (env == nullptr || hardware_buffer == nullptr || env->ExceptionCheck()) return nullptr;

  jclass clazz = env->FindClass("android/hardware/HardwareBuffer");
  if (clazz == nullptr) return nullptr;
  if (!env->IsInstanceOf(hardware_buffer, clazz)) {
    env->DeleteLocalRef(clazz);
    return nullptr;
  }

  jfieldID native_object = env->GetFieldID(clazz, "mNativeObject", "J");
  env->DeleteLocalRef(clazz);
  if (native_object == nullptr) return nullptr;

  const jlong handle = env->GetLongField(hardware_buffer, native_object);
  if (env->ExceptionCheck() || handle == 0) return nullptr;

  // This function returns a borrowed pointer, matching the NDK contract.  A
  // caller that outlives the Java HardwareBuffer must acquire its own ref.
  return HandleToBuffer(handle);
}

extern "C" jobject AHardwareBuffer_toHardwareBuffer(
    JNIEnv* env, AHardwareBuffer* hardware_buffer) {
  if (env == nullptr || hardware_buffer == nullptr || env->ExceptionCheck()) return nullptr;

  jclass clazz = env->FindClass("android/hardware/HardwareBuffer");
  if (clazz == nullptr) return nullptr;
  jmethodID constructor = env->GetMethodID(clazz, "<init>", "(J)V");
  if (constructor == nullptr) {
    env->DeleteLocalRef(clazz);
    return nullptr;
  }

  // The Java object owns this acquired reference.  If construction throws or
  // fails, balance the acquire before returning.
  AHardwareBuffer_acquire(hardware_buffer);
  jobject result = env->NewObject(
      clazz, constructor, reinterpret_cast<jlong>(hardware_buffer));
  if (result == nullptr) AHardwareBuffer_release(hardware_buffer);
  env->DeleteLocalRef(clazz);
  return result;
}

namespace darwin_art::hardware_buffer {

void HardwareBufferFinalizer(void* opaque) {
  if (opaque != nullptr) AHardwareBuffer_release(
      static_cast<AHardwareBuffer*>(opaque));
}

jlong HardwareBufferNativeCreate(JNIEnv*, jclass, jint width, jint height,
                                 jint format, jint layers, jlong usage) {
  AHardwareBuffer_Desc desc{
      .width = static_cast<uint32_t>(width),
      .height = static_cast<uint32_t>(height),
      .layers = static_cast<uint32_t>(layers),
      .format = static_cast<uint32_t>(format),
      .usage = static_cast<uint64_t>(usage),
      .stride = 0,
      .rfu0 = 0,
      .rfu1 = 0,
  };
  AHardwareBuffer* buffer = nullptr;
  return AHardwareBuffer_allocate(&desc, &buffer) == 0
             ? reinterpret_cast<jlong>(buffer)
             : 0;
}

jlong HardwareBufferNativeCreateFromGraphicBuffer(JNIEnv* env, jclass,
                                                  jobject) {
  ThrowUnsupported(env, "GraphicBuffer ownership conversion is not connected");
  return 0;
}

jlong HardwareBufferNativeGetFinalizer(JNIEnv*, jclass) {
  return reinterpret_cast<jlong>(&HardwareBufferFinalizer);
}

void HardwareBufferNativeWriteToParcel(JNIEnv* env, jclass, jlong, jobject) {
  ThrowUnsupported(env, "HardwareBuffer Parcel transport is not connected");
}

jlong HardwareBufferNativeReadFromParcel(JNIEnv* env, jclass, jobject) {
  ThrowUnsupported(env, "HardwareBuffer Parcel transport is not connected");
  return 0;
}

jboolean HardwareBufferNativeIsSupported(JNIEnv*, jclass, jint width,
                                         jint height, jint format, jint layers,
                                         jlong usage) {
  AHardwareBuffer_Desc desc{
      .width = static_cast<uint32_t>(width),
      .height = static_cast<uint32_t>(height),
      .layers = static_cast<uint32_t>(layers),
      .format = static_cast<uint32_t>(format),
      .usage = static_cast<uint64_t>(usage),
  };
  return AHardwareBuffer_isSupported(&desc) ? JNI_TRUE : JNI_FALSE;
}

jint HardwareBufferNativeGetWidth(JNIEnv*, jclass, jlong handle) {
  return static_cast<jint>(Describe(handle).width);
}

jint HardwareBufferNativeGetHeight(JNIEnv*, jclass, jlong handle) {
  return static_cast<jint>(Describe(handle).height);
}

jint HardwareBufferNativeGetFormat(JNIEnv*, jclass, jlong handle) {
  return static_cast<jint>(Describe(handle).format);
}

jint HardwareBufferNativeGetLayers(JNIEnv*, jclass, jlong handle) {
  return static_cast<jint>(Describe(handle).layers);
}

jlong HardwareBufferNativeGetUsage(JNIEnv*, jclass, jlong handle) {
  return static_cast<jlong>(Describe(handle).usage);
}

jlong HardwareBufferNativeEstimateSize(jlong handle) {
  // Compatibility estimate only: this does not report the backing
  // IOSurface allocation size when the provider's stride differs.
  const AHardwareBuffer_Desc desc = Describe(handle);
  uint32_t bytes_per_pixel = 4;
  if (desc.format == AHARDWAREBUFFER_FORMAT_R5G6B5_UNORM) bytes_per_pixel = 2;
  return static_cast<jlong>(desc.height) *
         static_cast<jlong>(desc.stride == 0 ? desc.width : desc.stride) *
         bytes_per_pixel * std::max<uint32_t>(1, desc.layers);
}

jlong HardwareBufferNativeGetId(jlong handle) { return handle; }

bool RegisterHardwareBufferNatives(JNIEnv* env) {
  JNINativeMethod methods[] = {
      {const_cast<char*>("nCreateHardwareBuffer"),
       const_cast<char*>("(IIIIJ)J"),
       reinterpret_cast<void*>(&HardwareBufferNativeCreate)},
      {const_cast<char*>("nCreateFromGraphicBuffer"),
       const_cast<char*>("(Landroid/graphics/GraphicBuffer;)J"),
       reinterpret_cast<void*>(&HardwareBufferNativeCreateFromGraphicBuffer)},
      {const_cast<char*>("nGetNativeFinalizer"), const_cast<char*>("()J"),
       reinterpret_cast<void*>(&HardwareBufferNativeGetFinalizer)},
      {const_cast<char*>("nWriteHardwareBufferToParcel"),
       const_cast<char*>("(JLandroid/os/Parcel;)V"),
       reinterpret_cast<void*>(&HardwareBufferNativeWriteToParcel)},
      {const_cast<char*>("nReadHardwareBufferFromParcel"),
       const_cast<char*>("(Landroid/os/Parcel;)J"),
       reinterpret_cast<void*>(&HardwareBufferNativeReadFromParcel)},
      {const_cast<char*>("nIsSupported"), const_cast<char*>("(IIIIJ)Z"),
       reinterpret_cast<void*>(&HardwareBufferNativeIsSupported)},
      {const_cast<char*>("nGetWidth"), const_cast<char*>("(J)I"),
       reinterpret_cast<void*>(&HardwareBufferNativeGetWidth)},
      {const_cast<char*>("nGetHeight"), const_cast<char*>("(J)I"),
       reinterpret_cast<void*>(&HardwareBufferNativeGetHeight)},
      {const_cast<char*>("nGetFormat"), const_cast<char*>("(J)I"),
       reinterpret_cast<void*>(&HardwareBufferNativeGetFormat)},
      {const_cast<char*>("nGetLayers"), const_cast<char*>("(J)I"),
       reinterpret_cast<void*>(&HardwareBufferNativeGetLayers)},
      {const_cast<char*>("nGetUsage"), const_cast<char*>("(J)J"),
       reinterpret_cast<void*>(&HardwareBufferNativeGetUsage)},
      {const_cast<char*>("nEstimateSize"), const_cast<char*>("(J)J"),
       reinterpret_cast<void*>(&HardwareBufferNativeEstimateSize)},
      {const_cast<char*>("nGetId"), const_cast<char*>("(J)J"),
       reinterpret_cast<void*>(&HardwareBufferNativeGetId)},
  };
  if (env == nullptr) return false;
  jclass clazz = env->FindClass("android/hardware/HardwareBuffer");
  if (clazz == nullptr) return false;
  const bool registered =
      env->RegisterNatives(clazz, methods, static_cast<jint>(std::size(methods))) ==
      JNI_OK;
  env->DeleteLocalRef(clazz);
  return registered;
}

}  // namespace darwin_art::hardware_buffer
