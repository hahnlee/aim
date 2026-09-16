#include "../../../runtime/framework/system/process_entry.h"
#include "../../../runtime/framework/am/application_binding.h"
#include <cstring>

namespace {
int mode;
int served;
jstring Resolve(JNIEnv* env, jclass, jstring) {
  return env->NewStringUTF("installed-record");
}
}
extern "C" int darwin_art_install_context_loader(JNIEnv*, jobject loader) {
  return loader == nullptr ? 4 : 0;
}
extern "C" bool darwin_art_surfaceflinger_service_start() { return mode != 3; }
namespace darwin_art::framework::app {
int FinishFrameworkRegistration(JNIEnv*, bool) { return 0; }
}
namespace darwin_art::framework::system {
bool StartKernelBinderService(JNIEnv*, jobject service_directory) {
  return service_directory != nullptr && mode != 4;
}
bool InitializeApplicationSharedMemory(JNIEnv* env) {
  if (mode != 2) return true;
  jclass error = env->FindClass("java/lang/IllegalStateException");
  env->ThrowNew(error, "initialization failed");
  env->DeleteLocalRef(error);
  return false;
}
}
namespace darwin_art::framework::am {
bool RegisterActivityManager(JNIEnv*, jclass endpoint, PackageResolver resolver) {
  return endpoint != nullptr && resolver == &Resolve;
}
}
namespace darwin_art {
int ServeBinderServiceEndpoint(JNIEnv*, jobject binder, const char* socket) {
  ++served;
  return binder != nullptr && std::strcmp(socket, "/test/explicit-socket") == 0 ? 23 : 71;
}
}
extern "C" JNIEXPORT jint JNICALL Java_SystemProcessEntryTest_run(
    JNIEnv* env, jclass, jint requested) {
  mode = requested;
  std::string classpath;
  if (!darwin_art::framework::system::BuildSystemClassPath(
          "/image with spaces", "/support/code.dex", &classpath) ||
      classpath != "/image with spaces/system/framework/services.jar:/support/code.dex") return 74;
  const std::string previous = classpath;
  const char* invalid_paths[] = {nullptr, "relative", "/", "/image:extra", "/image/.."};
  for (const char* invalid : invalid_paths) {
    if (darwin_art::framework::system::BuildSystemClassPath(
            invalid, "/support/code.dex", &classpath) || classpath != previous) return 75;
  }
  if (darwin_art::framework::system::BuildSystemClassPath(
          "/image", "/support:/extra", &classpath) || classpath != previous) return 76;
  served = 0;
  int result = darwin_art::framework::system::RunSystemProcess(
      env, mode == 1 ? nullptr : "/test/explicit-socket", &Resolve);
  if ((mode == 0 && served != 1) || (mode != 0 && served != 0)) return 72;
  return result;
}
