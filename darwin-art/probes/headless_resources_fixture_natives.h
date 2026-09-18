#pragma once

#include <jni.h>

namespace darwin_art_headless_fixture {

// Installs the fake AssetManager JNI table used by the headless fixture.
bool RegisterResourceNatives(JNIEnv* env);

}  // namespace darwin_art_headless_fixture
