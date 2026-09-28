#include "kernel_binder_client.h"

#include <binder/ProcessState.h>
#include <utils/Errors.h>

#include <cstdio>

namespace aim::framework::app {

bool StartApplicationBinderPool() {
  android::sp<android::ProcessState> process =
      android::ProcessState::initWithDriver("/dev/binder");
  if (process == nullptr) {
    std::fprintf(stderr,
                 "aim: application ProcessState initialization failed\n");
    return false;
  }
  // This is libbinder's normal userspace maximum. The Darwin Binder device
  // retains authority over actual worker requests and queue wakeups.
  if (process->setThreadPoolMaxThreadCount(15) != android::OK) {
    std::fprintf(stderr,
                 "aim: application Binder thread-pool configuration failed\n");
    return false;
  }
  process->startThreadPool();
  return true;
}

}  // namespace aim::framework::app
