#include "darwin_jni_shorty.h"
#include "darwin_runtime_adapters_internal.h"

namespace android {

int32_t ProxyThrowNew(void* context, void* clazz, const char* message) {
  auto* library = static_cast<ElfLibrary*>(context);
  JNIEnv* art_env = CurrentArtEnv();
  if (library == nullptr || art_env == nullptr || clazz == nullptr ||
      message == nullptr) {
    return AIM_JNI_ERR;
  }
  return art_env->ThrowNew(static_cast<jclass>(clazz), message);
}

}  // namespace android
