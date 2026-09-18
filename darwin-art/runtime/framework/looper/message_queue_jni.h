#pragma once
#include <jni.h>
namespace darwin_art::framework_system {
jlong message_queue_native_init(JNIEnv*, jclass);
void message_queue_native_destroy(JNIEnv*, jclass, jlong);
void message_queue_native_poll_once(JNIEnv*, jobject, jlong, jint);
void message_queue_native_wake(JNIEnv*, jclass, jlong);
jboolean message_queue_native_is_polling(JNIEnv*, jclass, jlong);
void message_queue_native_set_file_descriptor_events(JNIEnv*, jclass, jlong,
                                                     jint, jint);
// Resolves the exact native Looper retained by an Android MessageQueue. The
// Lookup admits the queue identity against concurrent nativeDestroy. The
// returned Looper is retained by the provider's process-lifetime thread
// association; no queue-object address or ownership escapes this interface.
void* message_queue_looper(JNIEnv*, jobject);
}
