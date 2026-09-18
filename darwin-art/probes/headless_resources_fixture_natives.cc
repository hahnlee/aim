#include "headless_resources_fixture_natives.h"

#include <cstdint>
#include <iterator>

namespace {

struct DarwinAssetManager {};

struct DarwinTheme {
  explicit DarwinTheme(DarwinAssetManager* owner) : assets(owner) {}

  DarwinAssetManager* assets;
};

void AssetManagerFinalizer(void* assets) {
  delete static_cast<DarwinAssetManager*>(assets);
}

jlong AssetManagerCreate(JNIEnv*, jclass) {
  return reinterpret_cast<std::uintptr_t>(new DarwinAssetManager());
}

void AssetManagerDestroy(JNIEnv*, jclass, jlong handle) {
  AssetManagerFinalizer(reinterpret_cast<void*>(
      static_cast<std::uintptr_t>(handle)));
}

void ThemeFinalizer(void* theme) { delete static_cast<DarwinTheme*>(theme); }

jlong AssetManagerGetThemeFreeFunction(JNIEnv*, jclass) {
  return reinterpret_cast<std::uintptr_t>(&ThemeFinalizer);
}

jlong AssetManagerThemeCreate(JNIEnv*, jclass, jlong assets_handle) {
  auto* assets = reinterpret_cast<DarwinAssetManager*>(
      static_cast<std::uintptr_t>(assets_handle));
  return reinterpret_cast<std::uintptr_t>(new DarwinTheme(assets));
}

void AssetManagerThemeApplyStyle(JNIEnv*, jclass, jlong, jlong, jint,
                                 jboolean) {}
void AssetManagerThemeCopy(JNIEnv*, jclass, jlong, jlong, jlong, jlong) {}

jint AssetManagerThemeGetAttributeValue(JNIEnv*, jclass, jlong, jlong, jint,
                                        jobject, jboolean) {
  return 0;
}

jint AssetManagerThemeGetChangingConfigurations(JNIEnv*, jclass, jlong) {
  return 0;
}

void AssetManagerThemeRebase(JNIEnv*, jclass, jlong, jlong, jintArray,
                             jbooleanArray, jint) {}

void AssetManagerApplyStyle(JNIEnv*, jclass, jlong, jlong, jint, jint, jlong,
                            jintArray, jlong, jlong) {
  // Java's TypedArray storage is zero-initialized. Leaving it untouched is an
  // empty style result, so every framework lookup observes its documented
  // default while the real framework-res table parser is brought up.
}

void AssetManagerApplyStyleWithArray(JNIEnv*, jclass, jlong, jlong, jint, jint,
                                     jlong, jintArray, jintArray, jintArray) {}

jboolean AssetManagerResolveAttrs(JNIEnv*, jclass, jlong, jlong, jint, jint,
                                  jintArray, jintArray, jintArray,
                                  jintArray) {
  return JNI_TRUE;
}

jboolean AssetManagerRetrieveAttributes(JNIEnv*, jclass, jlong, jlong,
                                         jintArray, jintArray, jintArray) {
  return JNI_TRUE;
}

void AssetManagerSetApkAssets(JNIEnv*, jclass, jlong, jobjectArray, jboolean,
                              jboolean) {}

void AssetManagerSetConfiguration(
    JNIEnv*, jclass, jlong, jint, jint, jstring, jobjectArray, jint, jint,
    jint, jint, jint, jint, jint, jint, jint, jint, jint, jint, jint, jint,
    jint, jint, jboolean) {}

jobjectArray AssetManagerGetLocales(JNIEnv* env, jclass, jlong, jboolean) {
  jclass string_class = env->FindClass("java/lang/String");
  jobjectArray result = string_class == nullptr
                            ? nullptr
                            : env->NewObjectArray(0, string_class, nullptr);
  env->DeleteLocalRef(string_class);
  return result;
}

bool Register(JNIEnv* env, const char* class_name, JNINativeMethod* methods,
              jint method_count) {
  jclass klass = env->FindClass(class_name);
  if (klass == nullptr) {
    return false;
  }
  const bool registered =
      env->RegisterNatives(klass, methods, method_count) == JNI_OK;
  env->DeleteLocalRef(klass);
  return registered;
}

}  // namespace

namespace darwin_art_headless_fixture {

bool RegisterResourceNatives(JNIEnv* env) {
  JNINativeMethod methods[] = {
      {const_cast<char*>("nativeCreate"), const_cast<char*>("()J"),
       reinterpret_cast<void*>(&AssetManagerCreate)},
      {const_cast<char*>("nativeDestroy"), const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&AssetManagerDestroy)},
      {const_cast<char*>("nativeGetThemeFreeFunction"),
       const_cast<char*>("()J"),
       reinterpret_cast<void*>(&AssetManagerGetThemeFreeFunction)},
      {const_cast<char*>("nativeThemeCreate"), const_cast<char*>("(J)J"),
       reinterpret_cast<void*>(&AssetManagerThemeCreate)},
      {const_cast<char*>("nativeThemeApplyStyle"),
       const_cast<char*>("(JJIZ)V"),
       reinterpret_cast<void*>(&AssetManagerThemeApplyStyle)},
      {const_cast<char*>("nativeThemeCopy"), const_cast<char*>("(JJJJ)V"),
       reinterpret_cast<void*>(&AssetManagerThemeCopy)},
      {const_cast<char*>("nativeThemeGetAttributeValue"),
       const_cast<char*>("(JJILandroid/util/TypedValue;Z)I"),
       reinterpret_cast<void*>(&AssetManagerThemeGetAttributeValue)},
      {const_cast<char*>("nativeThemeGetChangingConfigurations"),
       const_cast<char*>("(J)I"),
       reinterpret_cast<void*>(&AssetManagerThemeGetChangingConfigurations)},
      {const_cast<char*>("nativeThemeRebase"),
       const_cast<char*>("(JJ[I[ZI)V"),
       reinterpret_cast<void*>(&AssetManagerThemeRebase)},
      {const_cast<char*>("nativeApplyStyle"),
       const_cast<char*>("(JJIIJ[IJJ)V"),
       reinterpret_cast<void*>(&AssetManagerApplyStyle)},
      {const_cast<char*>("nativeApplyStyleWithArray"),
       const_cast<char*>("(JJIIJ[I[I[I)V"),
       reinterpret_cast<void*>(&AssetManagerApplyStyleWithArray)},
      {const_cast<char*>("nativeResolveAttrs"),
       const_cast<char*>("(JJII[I[I[I[I)Z"),
       reinterpret_cast<void*>(&AssetManagerResolveAttrs)},
      {const_cast<char*>("nativeRetrieveAttributes"),
       const_cast<char*>("(JJ[I[I[I)Z"),
       reinterpret_cast<void*>(&AssetManagerRetrieveAttributes)},
      {const_cast<char*>("nativeSetApkAssets"),
       const_cast<char*>("(J[Landroid/content/res/ApkAssets;ZZ)V"),
       reinterpret_cast<void*>(&AssetManagerSetApkAssets)},
      {const_cast<char*>("nativeSetConfiguration"),
       const_cast<char*>(
           "(JIILjava/lang/String;[Ljava/lang/String;IIIIIIIIIIIIIIIIZ)V"),
       reinterpret_cast<void*>(&AssetManagerSetConfiguration)},
      {const_cast<char*>("nativeGetLocales"),
       const_cast<char*>("(JZ)[Ljava/lang/String;"),
       reinterpret_cast<void*>(&AssetManagerGetLocales)},
  };
  return Register(env, "android/content/res/AssetManager", methods,
                  static_cast<jint>(std::size(methods)));
}

}  // namespace darwin_art_headless_fixture
