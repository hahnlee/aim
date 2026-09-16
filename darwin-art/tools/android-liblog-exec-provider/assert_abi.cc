#include <android/log.h>
#include <cstddef>
#include <cstdint>

extern "C" int darwin_art_bionic_vsnprintf(char*, size_t, const char*, const void*);

// Only the calling convention crosses this boundary. AOSP liblog continues to
// own condition formatting, fatal logging, the installed aborter and termination.
extern "C" [[noreturn]] void darwin_art_android_log_assert_captured(
    const char* condition, const char* tag, const char* format,
    uint8_t* gp, uint8_t* fp, uint8_t* stack) {
  if (!format) __android_log_assert(condition, tag, nullptr);
  struct {
    void* stack;
    void* gr_top;
    void* vr_top;
    int32_t gr_offs;
    int32_t vr_offs;
  } arguments{stack, gp + 64, fp + 128, -40, -128};
  char message[1024] = {};  // AOSP logger_write.cpp LOG_BUF_SIZE.
  darwin_art_bionic_vsnprintf(message, sizeof(message), format, &arguments);
  __android_log_assert(condition, tag, "%s", message);
}
