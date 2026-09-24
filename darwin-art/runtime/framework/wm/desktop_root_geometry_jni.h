#pragma once

#include <jni.h>

namespace darwin_art::framework::wm {
// App-process transport for the AppKit root geometry provider. Registration
// installs natives only; Ensure registers a visible root with ActivityTask
// after ActivityThread attach (non-Activity processes are ignored).
bool RegisterDesktopRootGeometryClient(JNIEnv* env);
bool EnsureDesktopRootGeometryClient(JNIEnv* env);
}  // namespace darwin_art::framework::wm
