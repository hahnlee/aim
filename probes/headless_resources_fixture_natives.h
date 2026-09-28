#pragma once

#include <jni.h>

namespace aim_headless_fixture {

// Installs the fake AssetManager JNI table used by the headless fixture.
bool RegisterResourceNatives(JNIEnv* env);

}  // namespace aim_headless_fixture
