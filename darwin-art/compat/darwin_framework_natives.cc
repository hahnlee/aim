#include "darwin_framework_natives.h"
#include "sensor/system_sensor_manager_jni.h"
#include "../runtime/framework/looper/message_queue_jni.h"
#include "../runtime/framework/wm/desktop_window_metadata.h"
#include "../runtime/framework/camera/camera_metadata_jni.h"
#include "window/hardware_buffer_jni.h"
#include "window/blast_buffer_queue_jni.h"
#include "window/sync_fence_jni.h"
#include "window/surface_control_jni.h"
#include "window/surface_jni.h"
#include "window/texture_view_jni.h"
#include "input/velocity_tracker_jni.h"
#include "media/framework_media_jni.h"
#include "media/image_reader_jni.h"
#include "media/audio_system_jni.h"
#include "media/audio_track_jni.h"
#include "media/media_extractor_jni.h"
#include "darwin_android_surface_texture.h"
#include "darwin_angle_egl.h"
#include "darwin_framework_system_natives.h"
#include "process/process_name.h"
#include "process/procfs_jni.h"
#include "process/scheduling_jni.h"
#include "memory/application_memory_jni.h"
#include "diagnostics/debugstore_jni.h"
#include "graphics/graphics_environment.h"
#include "../runtime/framework/app/activity_thread_jni.h"
#include "diagnostics/trace_jni.h"
#include "darwin_motion_event_natives.h"
#include "darwin_media_codec.h"
#include "darwin_security_trust.h"

#include <cstdint>
#include <ctime>
#include <cmath>
#include <cstring>
#include <cstdlib>
#include <cstdio>
#include <iostream>
#include <iterator>
#include <limits>
#include <memory>
#include <unordered_map>
#include <vector>
#include <utility>
#include <signal.h>
#include <unistd.h>

#include <android/surface_control.h>
#include <android/hardware_buffer.h>
#include <android/graphics/canvas.h>
#include "../_aosp/system/libziparchive/include/ziparchive/zip_archive.h"

#include <fcntl.h>
#include <unistd.h>

namespace android {
int register_android_util_Log(JNIEnv* env);
int register_com_android_internal_os_ClassLoaderFactory(JNIEnv* env);
}  // namespace android

namespace {

jbyteArray IncrementalFileSignature(JNIEnv*, jclass, jstring) {
  // Installed APKs live on ordinary macOS filesystems, not Linux IncFS.
  // Absence of an IncFS v4 signature is normal: the AOSP verifier then checks
  // an optional .idsig and proceeds to the APK's authenticated v3/v2 blocks.
  return nullptr;
}
jboolean IncrementalEnabled(JNIEnv*, jclass) { return JNI_FALSE; }
jboolean IncrementalFileDescriptor(JNIEnv*, jclass, jint) { return JNI_FALSE; }
jboolean IncrementalPath(JNIEnv*, jclass, jstring) { return JNI_FALSE; }

void NativeAllocationRegistryApplyFreeFunction(JNIEnv*, jclass,
                                                jlong free_function,
                                                jlong native_ptr) {
  if (free_function == 0 || native_ptr == 0) return;
  using FreeFunction = void (*)(void*);
  reinterpret_cast<FreeFunction>(static_cast<std::uintptr_t>(free_function))(
      reinterpret_cast<void*>(static_cast<std::uintptr_t>(native_ptr)));
}

void ProcessSendSignal(JNIEnv*, jclass, jint pid, jint signal) {
  if (pid > 0) (void)::kill(static_cast<pid_t>(pid), signal);
}

bool Register(JNIEnv* env, const char* class_name, JNINativeMethod* methods,
              jint method_count) {
  jclass klass = env->FindClass(class_name);
  if (klass == nullptr) {
    return false;
  }
  const bool registered =
      env->RegisterNatives(klass, methods, method_count) == JNI_OK;
  env->DeleteLocalRef(klass);
  return registered;
}

jint CharsetUtilsToModifiedUtf8Bytes(JNIEnv* env, jclass, jstring source,
                                     jint source_length, jlong destination,
                                     jint destination_offset,
                                     jint destination_length) {
  auto* bytes = reinterpret_cast<char*>(static_cast<std::uintptr_t>(destination));
  const jint worst_length = source_length * 4;
  if (destination_offset >= 0 &&
      destination_offset + worst_length < destination_length) {
    env->GetStringUTFRegion(source, 0, source_length,
                            bytes + destination_offset);
    return static_cast<jint>(
        std::strlen(bytes + destination_offset + source_length) +
        source_length);
  }

  const jint encoded_length = env->GetStringUTFLength(source);
  if (destination_offset >= 0 &&
      destination_offset + encoded_length < destination_length) {
    env->GetStringUTFRegion(source, 0, source_length,
                            bytes + destination_offset);
    return encoded_length;
  }
  return -encoded_length;
}

jstring CharsetUtilsFromModifiedUtf8Bytes(JNIEnv* env, jclass, jlong source,
                                          jint source_offset,
                                          jint source_length) {
  auto* bytes = reinterpret_cast<char*>(static_cast<std::uintptr_t>(source));
  const char saved = bytes[source_offset + source_length];
  bytes[source_offset + source_length] = '\0';
  jstring result = env->NewStringUTF(bytes + source_offset);
  bytes[source_offset + source_length] = saved;
  return result;
}

}  // namespace

// Keep the dynamic JNI fallback as well as the explicit table registration.
// Some framework threads resolve Process natives before the framework table is
// visible through their boot-class loader.
extern "C" JNIEXPORT void Java_android_os_Process_sendSignal(
    JNIEnv*, jclass, jint pid, jint signal) {
  if (pid > 0) {
    (void)::kill(static_cast<pid_t>(pid), signal);
  }
}
extern "C" JNIEXPORT void Java_android_os_Process_sendSignal__II(
    JNIEnv* env, jclass clazz, jint pid, jint signal) {
  Java_android_os_Process_sendSignal(env, clazz, pid, signal);
}
extern "C" JNIEXPORT jlong Java_android_os_Process_getElapsedCpuTime(
    JNIEnv* env, jclass clazz) {
  return darwin_art::framework_system::process_get_elapsed_cpu_time(env, clazz);
}
extern "C" JNIEXPORT void Java_android_os_Process_readProcLines(
    JNIEnv* env, jclass clazz, jstring file, jobjectArray req_fields,
    jlongArray out_fields) {
  darwin_art::process::ReadProcLines(env, clazz, file, req_fields, out_fields);
}
extern "C" JNIEXPORT jboolean Java_android_os_Process_readProcFile(
    JNIEnv* env, jclass clazz, jstring file, jintArray format,
    jobjectArray out_strings, jlongArray out_longs, jfloatArray out_floats) {
  return darwin_art::process::ReadProcFile(env, clazz, file, format,
                                           out_strings, out_longs, out_floats);
}
extern "C" JNIEXPORT jboolean Java_android_os_Process_parseProcLine(
    JNIEnv* env, jclass clazz, jbyteArray buffer, jint start_index,
    jint end_index, jintArray format, jobjectArray out_strings,
    jlongArray out_longs, jfloatArray out_floats) {
  return darwin_art::process::ParseProcLine(
      env, clazz, buffer, start_index, end_index, format, out_strings,
      out_longs, out_floats);
}

namespace darwin_art {

bool RegisterFrameworkSupportNatives(JNIEnv* env) {
  JNINativeMethod native_allocation_methods[] = {
      {const_cast<char*>("applyFreeFunction"), const_cast<char*>("(JJ)V"),
       reinterpret_cast<void*>(&NativeAllocationRegistryApplyFreeFunction)},
  };
  if (!Register(env, "libcore/util/NativeAllocationRegistry",
                native_allocation_methods,
                static_cast<jint>(std::size(native_allocation_methods)))) {
    return false;
  }
  // DarwinTrustManagerFactory is an optional app-side compatibility class,
  // not part of core-oj/core-libart. Do not poison boot registration when the
  // app support DEX has not been loaded yet; finish-phase registration will
  // install it once the class loader can resolve the class.
  jclass trust = env->FindClass(
      "dev/darwinart/security/DarwinTrustManagerFactory$DarwinTrustManager");
  if (trust == nullptr) {
    if (env->ExceptionCheck()) env->ExceptionClear();
    return true;
  }
  env->DeleteLocalRef(trust);
  return RegisterDarwinSecurityTrustNatives(env);
}

bool RegisterFrameworkNatives(JNIEnv* env) {
  if (android::register_android_app_Activity(env) != JNI_OK ||
      env->ExceptionCheck() ||
      android::register_android_app_ActivityThread(env) != JNI_OK ||
      env->ExceptionCheck()) return false;
  if (android::register_com_android_internal_os_ClassLoaderFactory(env) != JNI_OK ||
      env->ExceptionCheck()) return false;
  if (darwin_art::graphics::RegisterGraphicsEnvironment(env) != JNI_OK ||
      env->ExceptionCheck()) return false;
  if (android::register_com_android_internal_os_DebugStore(env) != JNI_OK ||
      env->ExceptionCheck()) return false;
  if (android::register_com_android_internal_os_ApplicationSharedMemory(env) != JNI_OK ||
      env->ExceptionCheck() ||
      android::register_android_app_PropertyInvalidatedCache(env) != JNI_OK ||
      env->ExceptionCheck()) {
    return false;
  }
#if defined(DARWIN_ART_REAL_GRAPHICS)
  // Preserve AndroidRuntime.cpp's dependency order: Binder registration
  // resolves StrictMode.onBinderStrictModePolicyChange(), whose class
  // initializer calls android.util.Log.isLoggable().  The complete upstream
  // Log table therefore belongs to this process-wide framework registration
  // phase, before Binder, rather than to the later AssetManager install step.
  if (android::register_android_util_Log(env) < 0 || env->ExceptionCheck()) {
    return false;
  }
#endif
  JNINativeMethod charset_utils_methods[] = {
      {const_cast<char*>("toModifiedUtf8Bytes"),
       const_cast<char*>("(Ljava/lang/String;IJII)I"),
       reinterpret_cast<void*>(&CharsetUtilsToModifiedUtf8Bytes)},
      {const_cast<char*>("fromModifiedUtf8Bytes"),
       const_cast<char*>("(JII)Ljava/lang/String;"),
       reinterpret_cast<void*>(&CharsetUtilsFromModifiedUtf8Bytes)},
  };
  if (!Register(env, "android/util/CharsetUtils", charset_utils_methods,
                static_cast<jint>(std::size(charset_utils_methods)))) {
    return false;
  }

  if (!darwin_art::media::RegisterMediaDrmNatives(env)) {
    return false;
  }

  if (!darwin_art::RegisterDarwinMediaCodecNatives(env)) {
    return false;
  }
  if (!darwin_art::media::RegisterMediaExtractorNatives(env)) {
    return false;
  }

  if (!RegisterMotionEventNatives(env)) {
    return false;
  }
  if (!darwin_art::input::RegisterVelocityTrackerNatives(env)) {
    return false;
  }
  if (!darwin_art::sensor::RegisterSystemSensorManagerNatives(env)) {
    return false;
  }

  using namespace framework_system;
  JNINativeMethod incremental_methods[] = {
      {const_cast<char*>("nativeIsEnabled"), const_cast<char*>("()Z"),
       reinterpret_cast<void*>(&IncrementalEnabled)},
      {const_cast<char*>("nativeIsV2Available"), const_cast<char*>("()Z"),
       reinterpret_cast<void*>(&IncrementalEnabled)},
      {const_cast<char*>("nativeIsIncrementalFd"), const_cast<char*>("(I)Z"),
       reinterpret_cast<void*>(&IncrementalFileDescriptor)},
      {const_cast<char*>("nativeIsIncrementalPath"), const_cast<char*>("(Ljava/lang/String;)Z"),
       reinterpret_cast<void*>(&IncrementalPath)},
      {const_cast<char*>("nativeUnsafeGetFileSignature"),
       const_cast<char*>("(Ljava/lang/String;)[B"),
       reinterpret_cast<void*>(&IncrementalFileSignature)},
  };
  if (!Register(env, "android/os/incremental/IncrementalManager",
                incremental_methods, static_cast<jint>(std::size(incremental_methods)))) {
    return false;
  }
  JNINativeMethod process_methods[] = {
      {const_cast<char*>("setArgV0Native"), const_cast<char*>("(Ljava/lang/String;)V"),
       reinterpret_cast<void*>(&darwin_art::process::SetArgV0)},
      {const_cast<char*>("setThreadPriority"), const_cast<char*>("(I)V"),
       reinterpret_cast<void*>(&darwin_art::process::SetThreadPriority)},
      {const_cast<char*>("setThreadPriority"), const_cast<char*>("(II)V"),
       reinterpret_cast<void*>(&darwin_art::process::SetThreadPriorityForTid)},
      {const_cast<char*>("getThreadPriority"), const_cast<char*>("(I)I"),
       reinterpret_cast<void*>(&darwin_art::process::GetThreadPriority)},
      {const_cast<char*>("getProcessGroup"), const_cast<char*>("(I)I"),
       reinterpret_cast<void*>(&darwin_art::process::GetProcessGroup)},
      {const_cast<char*>("setThreadGroup"), const_cast<char*>("(II)V"),
       reinterpret_cast<void*>(&darwin_art::process::SetThreadGroup)},
      {const_cast<char*>("getElapsedCpuTime"), const_cast<char*>("()J"),
       reinterpret_cast<void*>(&process_get_elapsed_cpu_time)},
      {const_cast<char*>("readProcLines"),
       const_cast<char*>("(Ljava/lang/String;[Ljava/lang/String;[J)V"),
       reinterpret_cast<void*>(&darwin_art::process::ReadProcLines)},
      {const_cast<char*>("readProcFile"),
       const_cast<char*>("(Ljava/lang/String;[I[Ljava/lang/String;[J[F)Z"),
       reinterpret_cast<void*>(&darwin_art::process::ReadProcFile)},
      {const_cast<char*>("parseProcLine"),
       const_cast<char*>("([BII[I[Ljava/lang/String;[J[F)Z"),
       reinterpret_cast<void*>(&darwin_art::process::ParseProcLine)},
      {const_cast<char*>("sendSignal"), const_cast<char*>("(II)V"),
       reinterpret_cast<void*>(&ProcessSendSignal)},
      {const_cast<char*>("getFreeMemory"), const_cast<char*>("()J"),
       reinterpret_cast<void*>(&darwin_art::process::GetFreeMemory)},
      {const_cast<char*>("getTotalMemory"), const_cast<char*>("()J"),
       reinterpret_cast<void*>(&darwin_art::process::GetTotalMemory)},
  };
  if (!Register(env, "android/os/Process", process_methods,
                static_cast<jint>(std::size(process_methods)))) {
    return false;
  }

  // Keep media registration order aligned with AndroidRuntime: the policy
  // session allocator precedes AudioTrack, followed by product strategy and
  // port-event clients. Each table is owned by its media subsystem TU.
  if (!darwin_art::media::RegisterAudioSystemNatives(env) ||
      !darwin_art::media::RegisterAudioTrackNatives(env) ||
      !darwin_art::media::RegisterAudioProductStrategyNatives(env) ||
      !darwin_art::media::RegisterAudioPortEventHandlerNatives(env)) {
    return false;
  }

  if (!darwin_art::window::RegisterSurfaceControlNatives(env)) {
    return false;
  }

  if (!darwin_art::window::RegisterBlastBufferQueueNatives(env)) {
    return false;
  }

  if (!darwin_art::media::RegisterImageReaderNatives(env)) return false;

  if (!darwin_art::hardware_buffer::RegisterHardwareBufferNatives(env)) {
    return false;
  }

  if (!darwin_art::window::RegisterSyncFenceNatives(env)) return false;

  if (!darwin_art::media::RegisterPublicFormatNatives(env)) {
    return false;
  }


  if (!darwin_art::window::RegisterSurfaceNatives(env)) {
    return false;
  }


  if (!RegisterDarwinSurfaceTextureNatives(env)) {
    return false;
  }

  if (!darwin_art::window::RegisterTextureViewNatives(env)) return false;

  if (!darwin_art::RegisterDarwinAngleEglNatives(env)) {
    return false;
  }

  JNINativeMethod message_queue_methods[] = {
      {const_cast<char*>("nativeInit"), const_cast<char*>("()J"),
       reinterpret_cast<void*>(&message_queue_native_init)},
      {const_cast<char*>("nativeDestroy"), const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&message_queue_native_destroy)},
      {const_cast<char*>("nativePollOnce"), const_cast<char*>("(JI)V"),
       reinterpret_cast<void*>(&message_queue_native_poll_once)},
      {const_cast<char*>("nativeWake"), const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&message_queue_native_wake)},
      {const_cast<char*>("nativeIsPolling"), const_cast<char*>("(J)Z"),
       reinterpret_cast<void*>(&message_queue_native_is_polling)},
      {const_cast<char*>("nativeSetFileDescriptorEvents"),
       const_cast<char*>("(JII)V"),
       reinterpret_cast<void*>(&message_queue_native_set_file_descriptor_events)},
  };
  if (!Register(env, "android/os/MessageQueue", message_queue_methods,
                static_cast<jint>(std::size(message_queue_methods)))) {
    return false;
  }

  if (!RegisterFrameworkAnimationNatives(env)) {
    return false;
  }

  JNINativeMethod event_log_methods[] = {
      {const_cast<char*>("writeEvent"),
       const_cast<char*>("(I[Ljava/lang/Object;)I"),
       reinterpret_cast<void*>(&event_log_write_event)},
  };
  if (!Register(env, "android/util/EventLog", event_log_methods,
                static_cast<jint>(std::size(event_log_methods)))) {
    return false;
  }

#if !defined(DARWIN_ART_REAL_GRAPHICS)
  JNINativeMethod log_methods[] = {
      {const_cast<char*>("isLoggable"),
       const_cast<char*>("(Ljava/lang/String;I)Z"),
       reinterpret_cast<void*>(&log_is_loggable)},
      {const_cast<char*>("println_native"),
       const_cast<char*>(
           "(IILjava/lang/String;Ljava/lang/String;)I"),
       reinterpret_cast<void*>(&log_println)},
  };
  if (!Register(env, "android/util/Log", log_methods,
                static_cast<jint>(std::size(log_methods)))) {
    return false;
  }
#endif

  tracing_perfetto::registerWithPerfetto(false);
  if (android::register_android_os_Trace(env) != JNI_OK || env->ExceptionCheck()) {
    return false;
  }

  JNINativeMethod system_clock_methods[] = {
      {const_cast<char*>("currentThreadTimeMillis"), const_cast<char*>("()J"),
       reinterpret_cast<void*>(&system_clock_current_thread_time_millis)},
      {const_cast<char*>("elapsedRealtime"), const_cast<char*>("()J"),
       reinterpret_cast<void*>(&system_clock_elapsed_realtime)},
      {const_cast<char*>("elapsedRealtimeNanos"), const_cast<char*>("()J"),
       reinterpret_cast<void*>(&system_clock_elapsed_realtime_nanos)},
      {const_cast<char*>("uptimeMillis"), const_cast<char*>("()J"),
       reinterpret_cast<void*>(&system_clock_uptime_millis)},
      {const_cast<char*>("uptimeNanos"), const_cast<char*>("()J"),
       reinterpret_cast<void*>(&system_clock_uptime_nanos)},
  };
  if (!Register(env, "android/os/SystemClock", system_clock_methods,
                static_cast<jint>(std::size(system_clock_methods)))) {
    return false;
  }

  if (!RegisterFrameworkBinderNatives(env)) {
    return false;
  }

  if (!camera::RegisterCameraMetadataNatives(env)) {
    return false;
  }

  if (!RegisterFrameworkSqliteNatives(env)) {
    return false;
  }

  if (!RegisterFrameworkSystemPropertyNatives(env)) {
    return false;
  }

  return true;
}

}  // namespace darwin_art
