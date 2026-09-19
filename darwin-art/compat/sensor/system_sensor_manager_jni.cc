#include "system_sensor_manager_jni.h"
#include "sensor_provider.h"

#include <cerrno>
#include <cstdint>
#include <iterator>

namespace darwin_art::sensor {
namespace {

// The JNI table is the Android 16 frameworks/base
// core/jni/android_hardware_SensorManager.cpp contract. SystemSensorManager.java
// retains enumeration, listener policy and direct-channel validation. This
// provider currently maps no host HID devices to Android sensor types.
void Throw(JNIEnv* env, const char* type, const char* message) {
  jclass exception = env->FindClass(type);
  if (exception != nullptr) {
    env->ThrowNew(exception, message);
    env->DeleteLocalRef(exception);
  }
}

bool ValidManager(JNIEnv* env, jlong handle) {
  if (IsManagerIdentity(handle)) return true;
  Throw(env, "java/lang/IllegalStateException", "Invalid native SensorManager");
  return false;
}

void NativeClassInit(JNIEnv* env, jclass) {
  // Fail registration/use when the release-pinned Sensor value contract
  // changes. There is no field population until a host sensor is mapped.
  jclass sensor = env->FindClass("android/hardware/Sensor");
  if (sensor == nullptr) return;
  struct Field { const char* name; const char* signature; };
  constexpr Field fields[] = {
      {"mName", "Ljava/lang/String;"}, {"mVendor", "Ljava/lang/String;"},
      {"mVersion", "I"}, {"mHandle", "I"}, {"mMaxRange", "F"},
      {"mResolution", "F"}, {"mPower", "F"}, {"mMinDelay", "I"},
      {"mFifoReservedEventCount", "I"}, {"mFifoMaxEventCount", "I"},
      {"mStringType", "Ljava/lang/String;"},
      {"mRequiredPermission", "Ljava/lang/String;"},
      {"mMaxDelay", "I"}, {"mFlags", "I"},
  };
  for (const auto& field : fields) {
    if (env->GetFieldID(sensor, field.name, field.signature) == nullptr) break;
  }
  if (!env->ExceptionCheck()) {
    (void)env->GetMethodID(sensor, "setType", "(I)Z");
    (void)env->GetMethodID(sensor, "setId", "(I)V");
    (void)env->GetMethodID(sensor, "setUuid", "(JJ)V");
  }
  env->DeleteLocalRef(sensor);
}

jlong NativeCreate(JNIEnv*, jclass, jstring) {
  // Android's Java owner has no native destructor. The NDK sensor manager
  // identity is process-static and shared by every Context instance.
  return reinterpret_cast<jlong>(ManagerIdentity());
}

jboolean NativeGetSensorAtIndex(JNIEnv* env, jclass, jlong manager, jobject,
                                jint) {
  if (!ValidManager(env, manager)) return JNI_FALSE;
  if (MappedSensorCount() != 0) {
    Throw(env, "java/lang/IllegalStateException",
          "Mapped sensor translation is not installed");
    return JNI_FALSE;
  }
  return JNI_FALSE;  // Shared provider inventory is currently empty.
}

void NativeGetDynamicSensors(JNIEnv* env, jclass, jlong manager, jobject) {
  if (!ValidManager(env, manager)) return;
  if (MappedSensorCount() != 0) {
    Throw(env, "java/lang/IllegalStateException",
          "Mapped sensor translation is not installed");
  }
}

void NativeGetRuntimeSensors(JNIEnv* env, jclass, jlong manager, jint, jobject) {
  NativeGetDynamicSensors(env, nullptr, manager, nullptr);
}

jboolean NativeInjectionEnabled(JNIEnv* env, jclass, jlong manager) {
  if (!ValidManager(env, manager)) return JNI_FALSE;
  return JNI_FALSE;
}

jint NativeCreateDirectChannel(JNIEnv* env, jclass, jlong manager, jint, jlong,
                               jint, jint, jobject) {
  if (!ValidManager(env, manager)) return -EINVAL;
  return -ENODEV;
}

void NativeDestroyDirectChannel(JNIEnv* env, jclass, jlong manager, jint) {
  if (!ValidManager(env, manager)) return;
  Throw(env, "java/lang/IllegalArgumentException", "No sensor direct channel exists");
}

jint NativeConfigDirectChannel(JNIEnv* env, jclass, jlong manager, jint, jint,
                               jint) {
  if (!ValidManager(env, manager)) return -EINVAL;
  return -ENODEV;
}

jint NativeSetOperationParameter(JNIEnv* env, jclass, jlong manager, jint,
                                 jint, jfloatArray, jintArray) {
  if (!ValidManager(env, manager)) return -EINVAL;
  return -ENODEV;
}

jlong NativeInitBaseEventQueue(JNIEnv* env, jclass, jlong manager, jobject,
                               jobject, jstring, jint, jstring, jstring) {
  if (!ValidManager(env, manager)) return 0;
  Throw(env, "java/lang/RuntimeException", "No mapped sensor event queue is available");
  return 0;
}

jint NativeEnableSensor(JNIEnv*, jclass, jlong, jint, jint, jint) { return -EINVAL; }
jint NativeDisableSensor(JNIEnv*, jclass, jlong, jint) { return -EINVAL; }
void NativeDestroySensorEventQueue(JNIEnv* env, jclass, jlong) {
  Throw(env, "java/lang/IllegalStateException", "No sensor event queue exists");
}
jint NativeFlushSensor(JNIEnv*, jclass, jlong) { return -EINVAL; }
jint NativeInjectSensorData(JNIEnv*, jclass, jlong, jint, jfloatArray, jint,
                            jlong) { return -EINVAL; }

bool Register(JNIEnv* env, const char* name, JNINativeMethod* methods,
              jint count) {
  jclass clazz = env->FindClass(name);
  if (clazz == nullptr) return false;
  const bool ok = env->RegisterNatives(clazz, methods, count) == JNI_OK;
  env->DeleteLocalRef(clazz);
  return ok && !env->ExceptionCheck();
}

}  // namespace

bool RegisterSystemSensorManagerNatives(JNIEnv* env) {
  JNINativeMethod manager_methods[] = {
      {const_cast<char*>("nativeClassInit"), const_cast<char*>("()V"), (void*)NativeClassInit},
      {const_cast<char*>("nativeCreate"), const_cast<char*>("(Ljava/lang/String;)J"), (void*)NativeCreate},
      {const_cast<char*>("nativeGetSensorAtIndex"), const_cast<char*>("(JLandroid/hardware/Sensor;I)Z"), (void*)NativeGetSensorAtIndex},
      {const_cast<char*>("nativeGetDefaultDeviceSensorAtIndex"), const_cast<char*>("(JLandroid/hardware/Sensor;I)Z"), (void*)NativeGetSensorAtIndex},
      {const_cast<char*>("nativeGetDynamicSensors"), const_cast<char*>("(JLjava/util/List;)V"), (void*)NativeGetDynamicSensors},
      {const_cast<char*>("nativeGetRuntimeSensors"), const_cast<char*>("(JILjava/util/List;)V"), (void*)NativeGetRuntimeSensors},
      {const_cast<char*>("nativeIsDataInjectionEnabled"), const_cast<char*>("(J)Z"), (void*)NativeInjectionEnabled},
      {const_cast<char*>("nativeIsReplayDataInjectionEnabled"), const_cast<char*>("(J)Z"), (void*)NativeInjectionEnabled},
      {const_cast<char*>("nativeIsHalBypassReplayDataInjectionEnabled"), const_cast<char*>("(J)Z"), (void*)NativeInjectionEnabled},
      {const_cast<char*>("nativeCreateDirectChannel"), const_cast<char*>("(JIJIILandroid/hardware/HardwareBuffer;)I"), (void*)NativeCreateDirectChannel},
      {const_cast<char*>("nativeDestroyDirectChannel"), const_cast<char*>("(JI)V"), (void*)NativeDestroyDirectChannel},
      {const_cast<char*>("nativeConfigDirectChannel"), const_cast<char*>("(JIII)I"), (void*)NativeConfigDirectChannel},
      {const_cast<char*>("nativeSetOperationParameter"), const_cast<char*>("(JII[F[I)I"), (void*)NativeSetOperationParameter},
  };
  JNINativeMethod queue_methods[] = {
      {const_cast<char*>("nativeInitBaseEventQueue"), const_cast<char*>("(JLjava/lang/ref/WeakReference;Landroid/os/MessageQueue;Ljava/lang/String;ILjava/lang/String;Ljava/lang/String;)J"), (void*)NativeInitBaseEventQueue},
      {const_cast<char*>("nativeEnableSensor"), const_cast<char*>("(JIII)I"), (void*)NativeEnableSensor},
      {const_cast<char*>("nativeDisableSensor"), const_cast<char*>("(JI)I"), (void*)NativeDisableSensor},
      {const_cast<char*>("nativeDestroySensorEventQueue"), const_cast<char*>("(J)V"), (void*)NativeDestroySensorEventQueue},
      {const_cast<char*>("nativeFlushSensor"), const_cast<char*>("(J)I"), (void*)NativeFlushSensor},
      {const_cast<char*>("nativeInjectSensorData"), const_cast<char*>("(JI[FIJ)I"), (void*)NativeInjectSensorData},
  };
  return Register(env, "android/hardware/SystemSensorManager", manager_methods,
                  std::size(manager_methods)) &&
         Register(env, "android/hardware/SystemSensorManager$BaseEventQueue",
                  queue_methods, std::size(queue_methods));
}

}  // namespace darwin_art::sensor
