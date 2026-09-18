#pragma once

#include <jni.h>

// Publishes the canonical ART-created application loader as both the process
// system loader and the current managed thread's context loader.
extern "C" int darwin_art_install_context_loader(JNIEnv* env,
                                                 jobject app_loader);
