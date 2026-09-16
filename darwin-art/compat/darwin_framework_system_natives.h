#pragma once

#include <jni.h>

namespace darwin_art::framework_system {

jint event_log_write_event(JNIEnv*, jclass, jint, jobjectArray);

jlong message_queue_native_init(JNIEnv*, jclass);
void message_queue_native_destroy(JNIEnv*, jclass, jlong);
void message_queue_native_poll_once(JNIEnv*, jobject, jlong, jint);
void message_queue_native_wake(JNIEnv*, jclass, jlong);
jboolean message_queue_native_is_polling(JNIEnv*, jclass, jlong);
void message_queue_native_set_file_descriptor_events(JNIEnv*, jclass, jlong,
                                                     jint, jint);
// Resolves the exact native Looper retained by an Android MessageQueue. The
// returned pointer remains owned by that queue.
void* message_queue_looper(JNIEnv*, jobject);

jboolean log_is_loggable(JNIEnv*, jclass, jstring, jint);
jint log_println(JNIEnv*, jclass, jint, jint, jstring, jstring);

jlong system_clock_current_thread_time_millis(JNIEnv*, jclass);
jlong system_clock_elapsed_realtime(JNIEnv*, jclass);
jlong system_clock_elapsed_realtime_nanos(JNIEnv*, jclass);
jlong system_clock_uptime_millis(JNIEnv*, jclass);
jlong system_clock_uptime_nanos(JNIEnv*, jclass);
jlong process_get_elapsed_cpu_time(JNIEnv*, jclass);

}  // namespace darwin_art::framework_system
