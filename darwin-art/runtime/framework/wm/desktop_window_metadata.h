#pragma once

#include <jni.h>
#include <cstddef>
#include <cstdint>

namespace darwin_art::framework::wm {

bool RegisterDesktopWindowMetadataClient(JNIEnv* env);
bool RegisterDesktopWindowMetadataReceiver(JNIEnv* env);
// Called by the app-side SurfaceControl provider after ActivityThread attach.
// Non-Activity processes are ignored; the Java receiver is idempotent.
bool EnsureDesktopWindowMetadataReceiver(JNIEnv* env);

}  // namespace darwin_art::framework::wm
