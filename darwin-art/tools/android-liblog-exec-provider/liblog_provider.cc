#include "darwin_art_liblog_provider.h"

#include <android/log.h>

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>

extern "C" int darwin_art_bionic_vsnprintf(char*, size_t, const char*,
                                             const void*);

struct AndroidArm64VaList {
  void* stack;
  void* gr_top;
  void* vr_top;
  int32_t gr_offs;
  int32_t vr_offs;
};

extern "C" int darwin_art_android_log_print(int priority, const char* tag,
                                             const char* format, ...);
extern "C" void darwin_art_android_log_assert(const char*, const char*, const char*, ...);
extern "C" int darwin_art_android_log_buf_print(int buffer, int priority,
                                                const char* tag,
                                                const char* format, ...);

extern "C" int darwin_art_android_log_buf_print_captured(
    int buffer, int priority, const char* tag, const char* format,
    uint8_t* gp_registers, uint8_t* fp_registers, uint8_t* caller_stack) {
  if (format == nullptr) return -1;
  AndroidArm64VaList arguments{caller_stack, gp_registers + 64,
                               fp_registers + 128, -32, -128};
  char message[4096];
  const int count = darwin_art_bionic_vsnprintf(
      message, sizeof(message), format, &arguments);
  if (count < 0) return count;
  return __android_log_buf_write(buffer, priority, tag, message);
}

extern "C" int darwin_art_android_log_print_captured(
    int priority, const char* tag, const char* format, uint8_t* gp_registers,
    uint8_t* fp_registers, uint8_t* caller_stack) {
  if (format == nullptr) return -1;
  AndroidArm64VaList arguments{caller_stack, gp_registers + 64,
                               fp_registers + 128, -40, -128};
  char message[4096];
  const int count = darwin_art_bionic_vsnprintf(
      message, sizeof(message), format, &arguments);
  if (count < 0) return count;
  return __android_log_write(priority, tag, message);
}

extern "C" int darwin_art_android_log_vprint(int priority, const char* tag,
                                                const char* format,
                                                const void* arguments) {
  if (format == nullptr || arguments == nullptr) return -1;
  char message[4096];
  const int count = darwin_art_bionic_vsnprintf(
      message, sizeof(message), format, arguments);
  if (count < 0) return count;
  return __android_log_write(priority, tag, message);
}

namespace {

int DarwinArtAndroidLogWrite(int priority, const char* tag,
                             const char* message) {
  return __android_log_write(priority, tag, message);
}

void DarwinArtAndroidLogWriteMessage(__android_log_message* message) {
  __android_log_write_log_message(message);
}

struct Entry {
  const char* name;
  uintptr_t address;
};

#define LIBLOG_ENTRY(symbol) {#symbol, reinterpret_cast<uintptr_t>(&symbol)}

extern "C" int darwin_art_android_log_error_write(int priority,
                                                    const char* tag,
                                                    int uid,
                                                    const char* message) {
  (void)uid;
  return __android_log_write(priority, tag, message);
}

// Exact sorted dynsym surface of the NDK r28c API 35 liblog.so stub.
const Entry kEntries[] = {
    {"__android_log_assert", reinterpret_cast<uintptr_t>(&darwin_art_android_log_assert)},
    {"__android_log_buf_print",
     reinterpret_cast<uintptr_t>(&darwin_art_android_log_buf_print)},
    LIBLOG_ENTRY(__android_log_buf_write),
    LIBLOG_ENTRY(__android_log_call_aborter),
    LIBLOG_ENTRY(__android_log_default_aborter),
    {"__android_log_error_write",
     reinterpret_cast<uintptr_t>(&darwin_art_android_log_error_write)},
    LIBLOG_ENTRY(__android_log_get_minimum_priority),
    LIBLOG_ENTRY(__android_log_is_loggable),
    LIBLOG_ENTRY(__android_log_is_loggable_len),
    LIBLOG_ENTRY(__android_log_logd_logger),
    {"__android_log_print",
     reinterpret_cast<uintptr_t>(&darwin_art_android_log_print)},
    LIBLOG_ENTRY(__android_log_set_aborter),
    LIBLOG_ENTRY(__android_log_set_default_tag),
    LIBLOG_ENTRY(__android_log_set_logger),
    LIBLOG_ENTRY(__android_log_set_minimum_priority),
    LIBLOG_ENTRY(__android_log_stderr_logger),
    {"__android_log_vprint",
     reinterpret_cast<uintptr_t>(&darwin_art_android_log_vprint)},
    {"__android_log_write", reinterpret_cast<uintptr_t>(&DarwinArtAndroidLogWrite)},
    {"__android_log_write_log_message",
     reinterpret_cast<uintptr_t>(&DarwinArtAndroidLogWriteMessage)},
};

static_assert(sizeof(kEntries) / sizeof(kEntries[0]) == DARWIN_ART_LIBLOG_PROVIDER_COUNT);

struct VersionEntry {
  const char* name;
  const char* version;
};
const VersionEntry kVersions[] = {
#include "../bionic-provider-namespace/generated/liblog_versions.inc"
};
static_assert(sizeof(kVersions) / sizeof(kVersions[0]) == DARWIN_ART_LIBLOG_PROVIDER_COUNT);

}  // namespace

extern "C" size_t darwin_art_liblog_provider_count() {
  return sizeof(kEntries) / sizeof(kEntries[0]);
}

extern "C" const char* darwin_art_liblog_provider_name(uint32_t ordinal) {
  return ordinal < darwin_art_liblog_provider_count() ? kEntries[ordinal].name : nullptr;
}

extern "C" uintptr_t darwin_art_liblog_provider_address(uint32_t ordinal) {
  return ordinal < darwin_art_liblog_provider_count() ? kEntries[ordinal].address : 0;
}

extern "C" uintptr_t darwin_art_liblog_provider_resolve(const char* symbol,
                                                          const char* version) {
  if (symbol == nullptr) return 0;
  if (version != nullptr && version[0] != '\0') {
    bool matches = false;
    for (const VersionEntry& entry : kVersions) {
      if (std::strcmp(entry.name, symbol) == 0 &&
          std::strcmp(entry.version, version) == 0) matches = true;
    }
    if (!matches) return 0;
  }
  for (const Entry& entry : kEntries) {
    if (std::strcmp(entry.name, symbol) == 0) return entry.address;
  }
  return 0;
}
