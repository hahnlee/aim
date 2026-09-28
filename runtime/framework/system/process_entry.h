#pragma once

#include <jni.h>
#include <string>

namespace aim::framework::system {

// Launch configuration is supplied by the host entry. No APK activity/graphics
// fixture is a prerequisite of this process.
bool BuildSystemClassPath(const char* image_root, const char* support_dex,
                          std::string* result);
int RunSystemProcess(JNIEnv* env, const char* socket_path);

}  // namespace aim::framework::system
