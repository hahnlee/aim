#include <android/dlext.h>
#include <android/log.h>
#include <dlfcn.h>

extern "C" __attribute__((visibility("default"))) int exercise_public_providers(const char* path) {
  void* first = dlopen(path, RTLD_NOW | RTLD_LOCAL);
  void* symbol = dlsym(first, "fixture_symbol");
  const char* error = dlerror();
  android_dlextinfo info{};
  void* second = android_dlopen_ext(path, RTLD_NOW | RTLD_LOCAL, &info);
  __android_log_print(ANDROID_LOG_INFO, "dso-fixture", "%p %s", symbol, error ?: "");
  int result = first ? dlclose(first) : -1;
  if (second) result += dlclose(second);
  return result;
}
