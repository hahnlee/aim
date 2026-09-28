#pragma once
#include <jni.h>

namespace aim::security {
// android.app.admin.SecurityLog natives (android_app_admin_SecurityLog.cpp).
bool RegisterSecurityLogNatives(JNIEnv* env);
}
