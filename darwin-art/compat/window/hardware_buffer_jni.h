// Darwin implementation of the public AHardwareBuffer/HardwareBuffer JNI
// bridge.  The Java handle used by this runtime is a raw Darwin
// AHardwareBuffer*, not the AOSP GraphicBufferWrapper type.

#ifndef DARWIN_ART_COMPAT_WINDOW_HARDWARE_BUFFER_JNI_H_
#define DARWIN_ART_COMPAT_WINDOW_HARDWARE_BUFFER_JNI_H_

#include <android/hardware_buffer_jni.h>
#include <jni.h>

namespace darwin_art::hardware_buffer {

// These are the native methods used by android.hardware.HardwareBuffer.  A
// Java mNativeObject value is always a raw Darwin AHardwareBuffer*.
void HardwareBufferFinalizer(void* opaque);
jlong HardwareBufferNativeCreate(JNIEnv* env, jclass clazz, jint width,
                                 jint height, jint format, jint layers,
                                 jlong usage);
jlong HardwareBufferNativeCreateFromGraphicBuffer(JNIEnv* env, jclass clazz,
                                                  jobject graphic_buffer);
jlong HardwareBufferNativeGetFinalizer(JNIEnv* env, jclass clazz);
void HardwareBufferNativeWriteToParcel(JNIEnv* env, jclass clazz, jlong handle,
                                       jobject parcel);
jlong HardwareBufferNativeReadFromParcel(JNIEnv* env, jclass clazz,
                                         jobject parcel);
jboolean HardwareBufferNativeIsSupported(JNIEnv* env, jclass clazz, jint width,
                                         jint height, jint format, jint layers,
                                         jlong usage);
jint HardwareBufferNativeGetWidth(JNIEnv* env, jclass clazz, jlong handle);
jint HardwareBufferNativeGetHeight(JNIEnv* env, jclass clazz, jlong handle);
jint HardwareBufferNativeGetFormat(JNIEnv* env, jclass clazz, jlong handle);
jint HardwareBufferNativeGetLayers(JNIEnv* env, jclass clazz, jlong handle);
jlong HardwareBufferNativeGetUsage(JNIEnv* env, jclass clazz, jlong handle);
jlong HardwareBufferNativeEstimateSize(jlong handle);
jlong HardwareBufferNativeGetId(jlong handle);

}  // namespace darwin_art::hardware_buffer

#endif  // DARWIN_ART_COMPAT_WINDOW_HARDWARE_BUFFER_JNI_H_
