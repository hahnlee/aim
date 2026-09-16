#include <android/log.h>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <csignal>
#include <sys/resource.h>
#include <sys/wait.h>
#include <unistd.h>

extern "C" uintptr_t darwin_art_liblog_provider_resolve(const char*, const char*);
extern "C" void darwin_art_test_log_assert(uintptr_t, const char*, const char*, const char*);
namespace {
int report_fd;
const char* expected_message;
void Logger(const __android_log_message* message) {
  if (!message || message->priority != ANDROID_LOG_FATAL ||
      !message->tag || std::strcmp(message->tag, "AssertABI") ||
      !message->message || std::strcmp(message->message, expected_message)) _exit(91);
  if (write(report_fd, "L", 1) != 1) _exit(92);
}
void Aborter(const char* message) {
  if (!message || std::strcmp(message, expected_message)) _exit(93);
  if (write(report_fd, "A", 1) != 1) _exit(94);
  // Returning deliberately verifies AOSP's unconditional abort after callback.
}
bool Run(const char* condition, const char* format, const char* expected) {
  int channel[2];
  if (pipe(channel)) return false;
  const auto address = darwin_art_liblog_provider_resolve("__android_log_assert", "LIBLOG");
  if (!address) { close(channel[0]); close(channel[1]); return false; }
  pid_t child = fork();
  if (child == 0) {
    close(channel[0]);
    report_fd = channel[1];
    expected_message = expected;
    struct rlimit limit{0, 0};
    if (setrlimit(RLIMIT_CORE, &limit)) _exit(95);
    __android_log_set_logger(Logger);
    __android_log_set_aborter(Aborter);
    darwin_art_test_log_assert(address, condition, "AssertABI", format);
    _exit(96);
  }
  close(channel[1]);
  if (child < 0) { close(channel[0]); return false; }
  char reports[3] = {};
  size_t size = 0;
  ssize_t count;
  while (size < sizeof(reports) &&
         (count = read(channel[0], reports + size, sizeof(reports) - size)) > 0)
    size += count;
  close(channel[0]);
  int status = 0;
  return waitpid(child, &status, 0) == child && WIFSIGNALED(status) &&
         WTERMSIG(status) == SIGABRT && size == 2 && !std::memcmp(reports, "LA", 2);
}
}
extern "C" int darwin_art_log_assert_smoke() {
  if (!Run("condition", "%d %d %d %d %d %d %.1f", "1 2 3 4 5 6 1.5") ||
      !Run("value%condition", nullptr, "Assertion failed: value%condition") ||
      !Run(nullptr, nullptr, "Unspecified assertion failed")) return 1;
  std::fprintf(stderr, "liblog assert: AAPCS64 registers/stack/FP, null format, logger/aborter/SIGABRT PASS\n");
  return 0;
}
