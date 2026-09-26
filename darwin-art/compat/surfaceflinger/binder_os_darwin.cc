#include "OS.h"

#include <pthread.h>
#include <fcntl.h>
#include <sys/socket.h>

namespace android {
void report_sysprop_change();
}

namespace android::binder::os {

void trace_begin(uint64_t, const char*) {}

void trace_end(uint64_t) {}

void trace_int(uint64_t, const char*, int32_t) {}

uint64_t get_trace_enabled_tags() {
  return 0;
}

uint64_t GetThreadId() {
  uint64_t thread_id = 0;
  pthread_threadid_np(nullptr, &thread_id);
  return thread_id;
}

// libbinder's SYSPROPS_TRANSACTION hook, as OS_android.cpp: run the process's
// property change callbacks (libutils misc.cpp).
bool report_sysprop_change() {
  android::report_sysprop_change();
  return true;
}

}  // namespace android::binder::os

extern "C" int darwin_art_accept4(int socket_fd, struct sockaddr* address,
                                   socklen_t* address_length, int) {
  const int accepted = accept(socket_fd, address, address_length);
  if (accepted < 0) return accepted;
  (void)fcntl(accepted, F_SETFD, FD_CLOEXEC);
  const int current_flags = fcntl(accepted, F_GETFL);
  if (current_flags >= 0) {
    (void)fcntl(accepted, F_SETFL, current_flags | O_NONBLOCK);
  }
  return accepted;
}
