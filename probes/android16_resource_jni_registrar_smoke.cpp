#include <jni.h>

#include <iterator>
#include <type_traits>

namespace android {
int register_android_content_AssetManager(JNIEnv*);
int register_android_content_res_ApkAssets(JNIEnv*);
int register_android_content_StringBlock(JNIEnv*);
int register_android_content_XmlBlock(JNIEnv*);
}  // namespace android

using Registrar = int (*)(JNIEnv*);
static_assert(std::is_same_v<decltype(&android::register_android_content_AssetManager), Registrar>);
static_assert(
    std::is_same_v<decltype(&android::register_android_content_res_ApkAssets), Registrar>);
static_assert(std::is_same_v<decltype(&android::register_android_content_StringBlock), Registrar>);
static_assert(std::is_same_v<decltype(&android::register_android_content_XmlBlock), Registrar>);

Registrar kAndroid16ResourceRegistrars[] = {
    &android::register_android_content_res_ApkAssets,
    &android::register_android_content_AssetManager,
    &android::register_android_content_StringBlock,
    &android::register_android_content_XmlBlock,
};

static_assert(std::size(kAndroid16ResourceRegistrars) == 4);
