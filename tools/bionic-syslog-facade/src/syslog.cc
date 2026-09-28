#include "aim_bionic_syslog.h"

#include <errno.h>
#include <pthread.h>
#include <stdint.h>
#include <string.h>
#include <sys/uio.h>
#include <unistd.h>

extern "C" int aim_bionic_vsnprintf(char*, size_t, const char*,
                                             const void*);
extern "C" void aim_bionic_errno_store(int);
extern "C" int __android_log_write(int, const char*, const char*);

namespace {

// Android 16 syslog.h and android/log.h values, pinned by audit.sh.
constexpr int kLogPriMask = 7;
constexpr int kLogErr = 3;
constexpr int kLogWarning = 4;
constexpr int kLogInfo = 6;
constexpr int kLogPerror = 0x20;
constexpr int kAndroidLogDebug = 3;
constexpr int kAndroidLogInfo = 4;
constexpr int kAndroidLogWarn = 5;
constexpr int kAndroidLogError = 6;
constexpr int kBionicEnotsup = 95;
constexpr size_t kMaximumGuestTag = 255;

struct AndroidVaList {
  void* stack;
  void* gr_top;
  void* vr_top;
  int32_t gr_offs;
  int32_t vr_offs;
};

pthread_mutex_t g_state_lock = PTHREAD_MUTEX_INITIALIZER;
const char* g_log_tag;
int g_options;
char g_default_tag[kMaximumGuestTag + 1];
bool g_activated;

int AndroidPriority(int priority) {
  priority &= kLogPriMask;
  if (priority <= kLogErr) return kAndroidLogError;
  if (priority == kLogWarning) return kAndroidLogWarn;
  if (priority <= kLogInfo) return kAndroidLogInfo;
  return kAndroidLogDebug;
}

void WritePerror(const char* tag, const char* line, int required) {
  const bool has_newline =
      required > 0 && required < 1024 && line[required - 1] == '\n';
  const char suffix = '\n';
  iovec vectors[] = {
      {const_cast<char*>(tag), strlen(tag)},
      {const_cast<char*>(": "), 2},
      {const_cast<char*>(line), strlen(line)},
      {const_cast<char*>(&suffix), has_newline ? size_t{0} : size_t{1}},
  };
  (void)writev(STDERR_FILENO, vectors, 4);
}

}  // namespace

extern "C" int aim_bionic_syslog_activate(
    const char* guest_program_tag) {
  const int saved_errno = errno;
  if (guest_program_tag == nullptr) {
    errno = saved_errno;
    return -1;
  }
  const size_t length = strnlen(guest_program_tag, kMaximumGuestTag + 1);
  if (length == 0 || length > kMaximumGuestTag) {
    errno = saved_errno;
    return -1;
  }
  (void)pthread_mutex_lock(&g_state_lock);
  if (g_activated) {
    (void)pthread_mutex_unlock(&g_state_lock);
    errno = saved_errno;
    return -1;
  }
  memcpy(g_default_tag, guest_program_tag, length + 1);
  g_activated = true;
  (void)pthread_mutex_unlock(&g_state_lock);
  errno = saved_errno;
  return 0;
}

extern "C" void aim_bionic_openlog(const char* ident, int option,
                                            int /* facility */) {
  const int saved_errno = errno;
  (void)pthread_mutex_lock(&g_state_lock);
  g_log_tag = ident;
  g_options = option;
  (void)pthread_mutex_unlock(&g_state_lock);
  errno = saved_errno;
}

extern "C" void aim_bionic_closelog() {
  const int saved_errno = errno;
  (void)pthread_mutex_lock(&g_state_lock);
  g_log_tag = nullptr;
  g_options = 0;
  (void)pthread_mutex_unlock(&g_state_lock);
  errno = saved_errno;
}

extern "C" void aim_bionic_syslog_captured(
    int priority, const char* format, const uint64_t* gp, const uint8_t* fp,
    uint8_t* stack) {
  const int saved_errno = errno;
  if (format == nullptr || gp == nullptr || fp == nullptr || stack == nullptr) {
    errno = saved_errno;
    return;
  }

  AndroidVaList arguments{
      stack,
      const_cast<uint64_t*>(gp) + 8,
      const_cast<uint8_t*>(fp) + 128,
      -48,  // x2 is the first anonymous GP argument.
      -128,
  };
  char line[1024];
  const int required =
      aim_bionic_vsnprintf(line, sizeof(line), format, &arguments);
  if (required < 0) {
    errno = saved_errno;
    return;
  }

  (void)pthread_mutex_lock(&g_state_lock);
  const char* tag = g_log_tag;
  if (tag == nullptr) {
    if (!g_activated) {
      (void)pthread_mutex_unlock(&g_state_lock);
      aim_bionic_errno_store(kBionicEnotsup);
      errno = saved_errno;
      return;
    }
    tag = g_default_tag;
  }
  const int options = g_options;
  (void)__android_log_write(AndroidPriority(priority), tag, line);
  if ((options & kLogPerror) != 0) WritePerror(tag, line, required);
  (void)pthread_mutex_unlock(&g_state_lock);
  errno = saved_errno;
}

extern "C" int aim_bionic_setlogmask(int mask) {
  static int current = 0xff;
  const int previous = current;
  if (mask != 0) current = mask;
  return previous;
}

extern "C" AimBionicSyslogFunction aim_bionic_syslog_resolve(
    const char* soname, const char* symbol, const char* version) {
  if (soname == nullptr || symbol == nullptr || version == nullptr ||
      strcmp(soname, "libc.so") != 0 || strcmp(version, "LIBC") != 0) {
    return nullptr;
  }
  if (strcmp(symbol, "closelog") == 0) {
    return reinterpret_cast<AimBionicSyslogFunction>(
        aim_bionic_closelog);
  }
  if (strcmp(symbol, "openlog") == 0) {
    return reinterpret_cast<AimBionicSyslogFunction>(
        aim_bionic_openlog);
  }
  if (strcmp(symbol, "setlogmask") == 0) {
    return reinterpret_cast<AimBionicSyslogFunction>(
        aim_bionic_setlogmask);
  }
  if (strcmp(symbol, "syslog") == 0) {
    return reinterpret_cast<AimBionicSyslogFunction>(
        aim_bionic_syslog);
  }
  return nullptr;
}

extern "C" const char* aim_bionic_syslog_capability(
    const char* capability) {
  if (capability == nullptr) return "invalid-capability";
  if (strcmp(capability, "android-aapcs64-varargs-capture") == 0 ||
      strcmp(capability, "aosp-liblog-write") == 0 ||
      strcmp(capability, "log-perror") == 0 ||
      strcmp(capability, "owned-guest-default-tag") == 0 ||
      strcmp(capability, "pinned-libcxx-percent-s") == 0) {
    return "supported";
  }
  return "unsupported";
}
