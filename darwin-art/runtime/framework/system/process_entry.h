#pragma once

#include <jni.h>
#include <string>

namespace darwin_art::framework::system {

// Launch configuration and installed-record transport are supplied by the host
// entry. No APK activity/graphics fixture is a prerequisite of this process.
using InstalledRecordResolver = jstring (*)(JNIEnv*, jclass, jstring);
bool BuildSystemClassPath(const char* image_root, const char* support_dex,
                          std::string* result);
int RunSystemProcess(JNIEnv* env, const char* socket_path,
                     InstalledRecordResolver resolver);

}  // namespace darwin_art::framework::system
