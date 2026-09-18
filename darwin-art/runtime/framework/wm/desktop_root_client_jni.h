#pragma once
#include <jni.h>
#include <memory>

namespace darwin_art::window { class DesktopRootEvents; }

namespace darwin_art::framework::wm {
bool RegisterDesktopRootClient(JNIEnv* env);
bool EnsureDesktopRootClient(JNIEnv* env, jobject input_channel);
// Pin only the exact native target already acquired by this client. No
// process/current-window lookup and no Android focus authority is supplied.
std::shared_ptr<window::DesktopRootEvents> RetainDesktopRootClientTarget(
    jlong target);
// Retryable shutdown gate; queued AppKit cleanup never waits on ART.
bool CloseDesktopRootClientAdmission(JNIEnv* env);
bool PollDesktopRootClientQuiesced(JNIEnv* env);
bool ClearDesktopRootClientReferences(JNIEnv* env);
}
