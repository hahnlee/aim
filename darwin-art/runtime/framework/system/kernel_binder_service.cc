#include "kernel_binder_service.h"

#include <binder/IPCThreadState.h>
#include <binder/ProcessState.h>

#include <cstdio>

#include "android_util_Binder.h"

namespace darwin_art::framework::system {

bool StartKernelBinderService(JNIEnv* env, jobject service_directory) {
  if (env == nullptr || service_directory == nullptr || env->ExceptionCheck()) {
    std::fprintf(stderr, "darwin-art: invalid kernel Binder service directory\n");
    return false;
  }
  android::sp<android::IBinder> binder =
      android::ibinderForJavaObject(env, service_directory);
  if (binder == nullptr || binder->localBinder() == nullptr ||
      env->ExceptionCheck()) {
    std::fprintf(stderr, "darwin-art: service directory is not a local Binder\n");
    return false;
  }

  android::sp<android::ProcessState> process =
      android::ProcessState::initWithDriver("/dev/binder");
  if (process == nullptr) {
    std::fprintf(stderr, "darwin-art: ProcessState initialization failed\n");
    return false;
  }
  android::sp<android::BBinder> context =
      android::sp<android::BBinder>::fromExisting(binder->localBinder());
  android::IPCThreadState::self()->setTheContextObject(context);
  if (!process->becomeContextManager()) {
    std::fprintf(stderr, "darwin-art: becomeContextManager failed\n");
    return false;
  }
  if (process->setThreadPoolMaxThreadCount(4) != android::OK) {
    std::fprintf(stderr, "darwin-art: Binder thread-pool configuration failed\n");
    return false;
  }
  process->startThreadPool();
  return true;
}

}  // namespace darwin_art::framework::system
