#include <android/dlext.h>
#include <dlfcn.h>
#include <jni.h>
#include <stdint.h>

typedef int (*PluginValue)(int);

static int ExercisePlugin(int input) {
  static const char kPlugin[] = "libguest_libdl_plugin.so";
  void *first = dlopen(kPlugin, RTLD_NOW | RTLD_LOCAL);
  if (first == NULL)
    return -1;
  void *second = dlopen(kPlugin, RTLD_NOW | RTLD_LOCAL);
  if (second == NULL || second != first)
    return -2;
  PluginValue value = (PluginValue)dlsym(first, "guest_plugin_value");
  if (value == NULL || value(input) != input + 7)
    return -3;
  if (dlsym(first, "guest_missing_symbol") != NULL)
    return -4;
  value = (PluginValue)dlsym(first, "guest_plugin_value");
  if (value == NULL || dlerror() == NULL || dlerror() != NULL)
    return -5;
  if (dlclose(first) != 0)
    return -6;
  value = (PluginValue)dlsym(second, "guest_plugin_value");
  if (value == NULL || value(input) != input + 7)
    return -7;
  if (dlclose(second) != 0)
    return -8;

  android_dlextinfo extinfo = {.flags = 0};
  if (android_dlopen_ext(kPlugin, RTLD_NOW | RTLD_LOCAL, &extinfo) != NULL ||
      dlerror() == NULL || dlerror() != NULL) {
    return -9;
  }
  if (dlopen(kPlugin, RTLD_LAZY | RTLD_LOCAL) != NULL || dlerror() == NULL ||
      dlerror() != NULL) {
    return -10;
  }
  return input + 7;
}

JNIEXPORT jint JNICALL JNI_OnLoad(JavaVM *vm, void *reserved) {
  (void)vm;
  (void)reserved;
  return ExercisePlugin(35) == 42 ? JNI_VERSION_1_6 : JNI_ERR;
}

JNIEXPORT jint JNICALL Java_dev_aim_probe_GuestLibdlFixture_nativePlugin(
    JNIEnv *env, jclass clazz, jint input) {
  (void)env;
  (void)clazz;
  return ExercisePlugin(input);
}
