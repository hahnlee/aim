#include "framework_media_jni.h"

#include <iterator>

namespace {

void MediaDrmNativeInit(JNIEnv*, jclass) {}

jboolean MediaDrmIsCryptoSchemeSupported(JNIEnv*, jclass, jbyteArray,
                                         jstring, jint) {
  return JNI_FALSE;
}

jbyteArray MediaDrmGetSupportedCryptoSchemes(JNIEnv* env, jclass) {
  return env->NewByteArray(0);
}

jint PublicFormatNativeGetHalFormat(JNIEnv*, jclass, jint format) {
  switch (format) {
    case 0x100:       // JPEG
    case 0x101:       // DEPTH_POINT_CLOUD
    case 0x69656963:  // DEPTH_JPEG
    case 0x48454946:  // HEIC
    case 0x1005:      // JPEG_R
    case 0x1006:      // HEIC_ULTRAHDR
      return 0x21;    // HAL_PIXEL_FORMAT_BLOB
    case 0x44363159:  // DEPTH16
      return 0x20363159;  // HAL_PIXEL_FORMAT_Y16
    case 0x20:        // RAW_SENSOR
    case 0x1002:      // RAW_DEPTH
      return 0x20;    // HAL_PIXEL_FORMAT_RAW16
    case 0x1003:      // RAW_DEPTH10
      return 0x25;    // HAL_PIXEL_FORMAT_RAW10
    default:
      return format;
  }
}

jint PublicFormatNativeGetHalDataspace(JNIEnv*, jclass, jint format) {
  switch (format) {
    case 0x100:       // JPEG
    case 0x23:        // YUV_420_888
    case 0x11:        // NV21
    case 0x32315659:  // YV12
      return 0x101;   // HAL_DATASPACE_V0_JFIF
    case 0x101:       // DEPTH_POINT_CLOUD
    case 0x44363159:  // DEPTH16
    case 0x1002:      // RAW_DEPTH
    case 0x1003:      // RAW_DEPTH10
      return 0x1000;  // HAL_DATASPACE_DEPTH
    case 0x69656963:  // DEPTH_JPEG
      return 0x1002;  // HAL_DATASPACE_DYNAMIC_DEPTH
    default:
      return 0;
  }
}

jint PublicFormatNativeGetPublicFormat(JNIEnv*, jclass, jint format,
                                       jint dataspace) {
  if (format == 0x21) {  // HAL_PIXEL_FORMAT_BLOB
    if (dataspace == 0x1000) return 0x101;
    if (dataspace == 0x1002) return 0x69656963;
    return 0x100;
  }
  if (format == 0x20) return dataspace == 0x1000 ? 0x1002 : 0x20;
  if (format == 0x25) return dataspace == 0x1000 ? 0x1003 : 0x25;
  if (format == 0x20363159)
    return dataspace == 0x1000 ? 0x44363159 : 0x20363159;
  return format;
}

bool Register(JNIEnv* env, const char* class_name, JNINativeMethod* methods,
              jint count) {
  if (env == nullptr) return false;
  jclass clazz = env->FindClass(class_name);
  if (clazz == nullptr) return false;
  const jint result = env->RegisterNatives(clazz, methods, count);
  env->DeleteLocalRef(clazz);
  return result == JNI_OK;
}

}  // namespace

namespace darwin_art::media {

bool RegisterMediaDrmNatives(JNIEnv* env) {
  JNINativeMethod methods[] = {
      {const_cast<char*>("native_init"), const_cast<char*>("()V"),
       reinterpret_cast<void*>(&MediaDrmNativeInit)},
      {const_cast<char*>("isCryptoSchemeSupportedNative"),
       const_cast<char*>("([BLjava/lang/String;I)Z"),
       reinterpret_cast<void*>(&MediaDrmIsCryptoSchemeSupported)},
      {const_cast<char*>("getSupportedCryptoSchemesNative"),
       const_cast<char*>("()[B"),
       reinterpret_cast<void*>(&MediaDrmGetSupportedCryptoSchemes)},
  };
  return Register(env, "android/media/MediaDrm", methods,
                  static_cast<jint>(std::size(methods)));
}

bool RegisterPublicFormatNatives(JNIEnv* env) {
  JNINativeMethod methods[] = {
      {const_cast<char*>("nativeGetHalFormat"), const_cast<char*>("(I)I"),
       reinterpret_cast<void*>(&PublicFormatNativeGetHalFormat)},
      {const_cast<char*>("nativeGetHalDataspace"), const_cast<char*>("(I)I"),
       reinterpret_cast<void*>(&PublicFormatNativeGetHalDataspace)},
      {const_cast<char*>("nativeGetPublicFormat"), const_cast<char*>("(II)I"),
       reinterpret_cast<void*>(&PublicFormatNativeGetPublicFormat)},
  };
  return Register(env, "android/media/PublicFormatUtils", methods,
                  static_cast<jint>(std::size(methods)));
}

}  // namespace darwin_art::media
