#pragma once

#include <jni.h>

namespace android {

// Version-pinned Binder JNI owns the native proxy/list layout. This query does
// not unlink or establish callback quiescence. A JNI exception is always failure.
enum class DeathRecipientPresence : jint { UNSUPPORTED = -1, ABSENT = 0, PRESENT = 1 };
DeathRecipientPresence QueryProxyDeathRecipient(JNIEnv* env, jobject binder,
                                               jobject recipient);

}  // namespace android
