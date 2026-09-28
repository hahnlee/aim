#pragma once
#include <jni.h>

namespace aim::window {

bool RegisterBlastBufferQueueNatives(JNIEnv* env);
// Called by the attached process owner before VM shutdown commitment. Closes
// admissions without waiting on Java callbacks; false preserves status 67.
bool QuiesceBlastBufferQueues(JNIEnv* env);

}  // namespace aim::window

// Returns one retained ANativeWindow reference owned by the BLAST queue
// represented by handle. The caller must release it with
// aim_android_ANativeWindow_release_if_managed(). Null means the queue
// is invalid or has no producer window.
extern "C" void* aim_android_blast_buffer_queue_acquire_native_window(
    jlong handle);
