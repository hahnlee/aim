#include "darwin_art_liblog_provider.h"

#include <android/log.h>

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <set>
#include <string>
#include <vector>

namespace {

std::vector<std::string> messages;

void Capture(const __android_log_message* message) {
  if (message == nullptr || message->struct_size < sizeof(__android_log_message)) std::abort();
  messages.emplace_back(message->message == nullptr ? "" : message->message);
}

template <typename T>
T Resolve(const char* name) {
  uintptr_t address = darwin_art_liblog_provider_resolve(name, nullptr);
  if (address == 0) {
    std::fprintf(stderr, "missing address: %s\n", name);
    std::abort();
  }
  return reinterpret_cast<T>(address);
}

}  // namespace

int main() {
  if (darwin_art_liblog_provider_count() != DARWIN_ART_LIBLOG_PROVIDER_COUNT) return 10;

  std::set<uintptr_t> addresses;
  for (uint32_t ordinal = 0; ordinal < DARWIN_ART_LIBLOG_PROVIDER_COUNT; ++ordinal) {
    const char* name = darwin_art_liblog_provider_name(ordinal);
    uintptr_t address = darwin_art_liblog_provider_address(ordinal);
    if (name == nullptr || address == 0 || !addresses.insert(address).second) return 11;
    if (darwin_art_liblog_provider_resolve(name, nullptr) != address) return 12;
    std::printf("%s\t0x%zx\n", name, static_cast<size_t>(address));
  }
  if (darwin_art_liblog_provider_name(DARWIN_ART_LIBLOG_PROVIDER_COUNT) != nullptr ||
      darwin_art_liblog_provider_address(DARWIN_ART_LIBLOG_PROVIDER_COUNT) != 0 ||
      darwin_art_liblog_provider_resolve("android_log_private", nullptr) != 0 ||
      darwin_art_liblog_provider_resolve("__android_log_write", "LIBLOG") == 0 ||
      darwin_art_liblog_provider_resolve("__android_log_assert", "LIBLOG") == 0 ||
      darwin_art_liblog_provider_resolve("__android_log_set_logger", "LIBLOG_R") == 0 ||
      darwin_art_liblog_provider_resolve("__android_log_set_logger", "LIBLOG") != 0 ||
      darwin_art_liblog_provider_resolve("__android_log_assert", "LIBLOG_R") != 0 ||
      darwin_art_liblog_provider_resolve("__android_log_write", "LIBC") != 0) {
    return 13;
  }

  using SetLogger = void (*)(__android_logger_function);
  using Write = int (*)(int, const char*, const char*);
  using Print = int (*)(int, const char*, const char*, ...);
  using WriteMessage = void (*)(__android_log_message*);

  Resolve<SetLogger>("__android_log_set_logger")(Capture);
  int write_result = Resolve<Write>("__android_log_write")(
      ANDROID_LOG_INFO, "DarwinArtProvider", "write-path");
  int print_result = Resolve<Print>("__android_log_print")(
      ANDROID_LOG_WARN, "DarwinArtProvider", "print-%d", 42);
  __android_log_message message{
      sizeof(__android_log_message), LOG_ID_MAIN, ANDROID_LOG_ERROR,
      "DarwinArtProvider", __FILE__, __LINE__, "logger-function-path"};
  Resolve<WriteMessage>("__android_log_write_log_message")(&message);

  if (write_result != 1 || print_result != 1 || messages.size() != 3 ||
      messages[0] != "write-path" || messages[1] != "print-42" ||
      messages[2] != "logger-function-path") {
    return 14;
  }
  Resolve<SetLogger>("__android_log_set_logger")(
      Resolve<__android_logger_function>("__android_log_stderr_logger"));
  std::fprintf(stderr,
               "android-liblog-exec-provider: PASS symbols=19 unique=19 calls=3 private=denied\n");
  return 0;
}
