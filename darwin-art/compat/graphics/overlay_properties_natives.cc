#include "graphics/overlay_properties_natives.h"

namespace {

struct OverlayProperties {
  bool mixed_color_spaces = true;
};

void DestroyOverlayProperties(OverlayProperties* properties) {
  delete properties;
}

jlong GetDestructor(JNIEnv*, jclass) {
  return reinterpret_cast<jlong>(&DestroyOverlayProperties);
}

jlong CreateDefault(JNIEnv*, jclass) {
  return reinterpret_cast<jlong>(new OverlayProperties());
}

jboolean SupportsMixedColorSpaces(JNIEnv*, jclass, jlong native_object) {
  auto* properties = reinterpret_cast<OverlayProperties*>(native_object);
  return properties != nullptr && properties->mixed_color_spaces;
}

jboolean IsCombinationSupported(JNIEnv*, jclass, jlong native_object,
                                jint /*dataspace*/, jint format) {
  // OverlayProperties.getDefault() is the Android virtual-display fallback:
  // RGBA_8888 is the only advertised buffer format there.
  return native_object != 0 && format == 1;
}

void WriteToParcel(JNIEnv* env, jclass, jlong native_object, jobject parcel) {
  jclass parcel_class = env->GetObjectClass(parcel);
  jmethodID write_int =
      parcel_class == nullptr
          ? nullptr
          : env->GetMethodID(parcel_class, "writeInt", "(I)V");
  if (write_int != nullptr) {
    auto* properties = reinterpret_cast<OverlayProperties*>(native_object);
    env->CallVoidMethod(parcel, write_int,
                        properties != nullptr && properties->mixed_color_spaces);
  }
  env->DeleteLocalRef(parcel_class);
}

jlong ReadFromParcel(JNIEnv* env, jclass, jobject parcel) {
  jclass parcel_class = env->GetObjectClass(parcel);
  jmethodID read_int =
      parcel_class == nullptr
          ? nullptr
          : env->GetMethodID(parcel_class, "readInt", "()I");
  if (read_int == nullptr) {
    env->DeleteLocalRef(parcel_class);
    return 0;
  }
  auto* properties = new OverlayProperties();
  properties->mixed_color_spaces = env->CallIntMethod(parcel, read_int) != 0;
  env->DeleteLocalRef(parcel_class);
  if (env->ExceptionCheck()) {
    delete properties;
    return 0;
  }
  return reinterpret_cast<jlong>(properties);
}

jobjectArray GetLutProperties(JNIEnv*, jclass, jlong) { return nullptr; }

const JNINativeMethod kMethods[] = {
    {"nGetDestructor", "()J", reinterpret_cast<void*>(GetDestructor)},
    {"nCreateDefault", "()J", reinterpret_cast<void*>(CreateDefault)},
    {"nSupportMixedColorSpaces", "(J)Z",
     reinterpret_cast<void*>(SupportsMixedColorSpaces)},
    {"nIsCombinationSupported", "(JII)Z",
     reinterpret_cast<void*>(IsCombinationSupported)},
    {"nWriteOverlayPropertiesToParcel", "(JLandroid/os/Parcel;)V",
     reinterpret_cast<void*>(WriteToParcel)},
    {"nReadOverlayPropertiesFromParcel", "(Landroid/os/Parcel;)J",
     reinterpret_cast<void*>(ReadFromParcel)},
    {"nGetLutProperties", "(J)[Landroid/hardware/LutProperties;",
     reinterpret_cast<void*>(GetLutProperties)},
};

}  // namespace

namespace darwin_art::graphics {

bool RegisterOverlayPropertiesNatives(JNIEnv* env) {
  jclass clazz = env->FindClass("android/hardware/OverlayProperties");
  if (clazz == nullptr) return false;
  const jint status = env->RegisterNatives(
      clazz, kMethods, static_cast<jint>(sizeof(kMethods) / sizeof(kMethods[0])));
  env->DeleteLocalRef(clazz);
  return status == JNI_OK && !env->ExceptionCheck();
}

}  // namespace darwin_art::graphics
