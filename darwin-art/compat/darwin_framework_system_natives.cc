#include <sys/time.h>
#include "darwin_framework_system_natives.h"

#include "darwin_android_platform.h"
#include "darwin_android_time.h"

#include <sys/resource.h>

#include <chrono>
#include <atomic>
#include <cstdlib>
#include <cstdint>
#include <iostream>
#include <time.h>

namespace darwin_art::framework_system {
namespace {

jlong TimevalToMillis(const timeval& value) {
  return static_cast<jlong>(value.tv_sec) * 1000 + value.tv_usec / 1000;
}

}  // namespace

jlong process_get_elapsed_cpu_time(JNIEnv*, jclass) {
  rusage usage{};
  if (getrusage(RUSAGE_SELF, &usage) != 0) return 0;
  return TimevalToMillis(usage.ru_utime) + TimevalToMillis(usage.ru_stime);
}

jint event_log_write_event(JNIEnv* env, jclass, jint tag, jobjectArray values) {
  // ServiceManager latency diagnostics are optional on the host; preserve
  // the Java call contract without importing Android's kernel event log.
  // SurfaceView's AOSP callback trace is useful when diagnosing the generic
  // SurfaceHolder contract. Keep this opt-in and at the Android EventLog
  // boundary instead of adding application-specific hooks.
  if (tag == 60006 && values != nullptr &&
      std::getenv("DARWIN_ART_DEBUG_SURFACE_CALLBACKS") != nullptr) {
    std::cerr << "ART Android SurfaceView callback:";
    const jsize count = env->GetArrayLength(values);
    for (jsize index = 0; index < count; ++index) {
      jobject value = env->GetObjectArrayElement(values, index);
      if (value == nullptr) {
        std::cerr << " <null>";
        continue;
      }
      jclass value_class = env->GetObjectClass(value);
      jmethodID to_string =
          value_class == nullptr
              ? nullptr
              : env->GetMethodID(value_class, "toString", "()Ljava/lang/String;");
      jstring text = to_string == nullptr
                         ? nullptr
                         : static_cast<jstring>(env->CallObjectMethod(value, to_string));
      const char* chars = text == nullptr ? nullptr : env->GetStringUTFChars(text, nullptr);
      std::cerr << ' ' << (chars == nullptr ? "<?>" : chars);
      if (chars != nullptr) env->ReleaseStringUTFChars(text, chars);
      if (text != nullptr) env->DeleteLocalRef(text);
      if (value_class != nullptr) env->DeleteLocalRef(value_class);
      env->DeleteLocalRef(value);
      if (env->ExceptionCheck()) {
        env->ExceptionClear();
        break;
      }
    }
    std::cerr << '\n';
  }
  return 0;
}

jboolean log_is_loggable(JNIEnv*, jclass, jstring, jint priority) {
  constexpr jint kInfoPriority = 4;
  return priority >= kInfoPriority ? JNI_TRUE : JNI_FALSE;
}

jint log_println(JNIEnv* env, jclass, jint, jint, jstring, jstring message) {
  return message == nullptr ? 0 : env->GetStringLength(message);
}

jlong system_clock_uptime_nanos(JNIEnv*, jclass) {
  return darwin_art::AndroidUptimeNanos();
}

jlong system_clock_uptime_millis(JNIEnv* env, jclass klass) {
  return system_clock_uptime_nanos(env, klass) / 1'000'000;
}

jlong system_clock_elapsed_realtime_nanos(JNIEnv*, jclass) {
  return darwin_art::AndroidElapsedRealtimeNanos();
}

jlong system_clock_elapsed_realtime(JNIEnv* env, jclass klass) {
  return system_clock_elapsed_realtime_nanos(env, klass) / 1'000'000;
}

jlong system_clock_current_thread_time_millis(JNIEnv*, jclass) {
  timespec value{};
  if (clock_gettime(CLOCK_THREAD_CPUTIME_ID, &value) != 0) {
    return 0;
  }
  return static_cast<jlong>(value.tv_sec) * 1'000 + value.tv_nsec / 1'000'000;
}

// android_os_SystemClock.cpp: systemTime(SYSTEM_TIME_THREAD) / 1000.
jlong system_clock_current_thread_time_micro(JNIEnv*, jclass) {
  timespec value{};
  if (clock_gettime(CLOCK_THREAD_CPUTIME_ID, &value) != 0) {
    return 0;
  }
  return static_cast<jlong>(value.tv_sec) * 1'000'000 + value.tv_nsec / 1'000;
}

// android_os_SystemClock.cpp: wall-clock gettimeofday in microseconds.
jlong system_clock_current_time_micro(JNIEnv*, jclass) {
  timeval value{};
  gettimeofday(&value, nullptr);
  return static_cast<jlong>(value.tv_sec) * 1'000'000 + value.tv_usec;
}

}  // namespace darwin_art::framework_system
