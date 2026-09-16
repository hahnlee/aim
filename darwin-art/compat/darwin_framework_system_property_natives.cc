#include "darwin_framework_natives.h"

namespace android {
int register_android_os_SystemProperties(JNIEnv* env);
}

namespace darwin_art {

bool RegisterFrameworkSystemPropertyNatives(JNIEnv* env) {
  // AOSP owns parsing, CriticalNative handles and change callback registration.
  // The linked JNI archive reads the installed process property area and sends
  // writes through the original property-service client, not a Java-only map.
  return android::register_android_os_SystemProperties(env) == JNI_OK;
}

}  // namespace darwin_art
