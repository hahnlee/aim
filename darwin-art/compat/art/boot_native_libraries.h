#pragma once
#include <jni.h>

namespace darwin_art::runtime_art {
// Called by ART's boot-native phase with its owner thread in kNative.
bool RegisterBootNativeLibraries(JNIEnv* env);
}
namespace darwin_art::platform::art {
// Resolve the dedicated Mach-O image; retain AOSP JavaVM library ownership.
bool LoadOpenJdkBootLibrary(JNIEnv* env);
}
