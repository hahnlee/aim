#pragma once

#include <jni.h>

namespace darwin_art::framework_system {

jint event_log_write_event(JNIEnv*, jclass, jint, jobjectArray);


jboolean log_is_loggable(JNIEnv*, jclass, jstring, jint);
jint log_println(JNIEnv*, jclass, jint, jint, jstring, jstring);

jlong system_clock_current_thread_time_millis(JNIEnv*, jclass);
jlong system_clock_elapsed_realtime(JNIEnv*, jclass);
jlong system_clock_elapsed_realtime_nanos(JNIEnv*, jclass);
jlong system_clock_uptime_millis(JNIEnv*, jclass);
jlong system_clock_uptime_nanos(JNIEnv*, jclass);
jlong process_get_elapsed_cpu_time(JNIEnv*, jclass);

}  // namespace darwin_art::framework_system
