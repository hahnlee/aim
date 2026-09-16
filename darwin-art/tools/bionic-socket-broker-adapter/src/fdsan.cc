#include "darwin_art_bionic_socket_broker.h"
#include "fdsan.h"
#include "darwin_art_bionic_errno.h"
#include <cstdint>
#include <cstdio>
#include <cstdlib>

extern "C" int darwin_art_fdsan_exchange(int, uint64_t, uint64_t, uint64_t*);
extern "C" int darwin_art_fdsan_report_error_level();
extern "C" int darwin_art_bionic_socket_broker_close_unchecked(int);

namespace {
// Darwin diagnostics boundary: Android severity and warn-once state are owned
// by Rust; fatal errors abort, warnings are logged without killing the app.
// Android debuggerd's warning dump transport is not supplied here.
void Exchange(int fd, uint64_t expected, uint64_t next) {
  uint64_t actual = 0;
  if (darwin_art_fdsan_exchange(fd, expected, next, &actual) != 0) {
    const int level = darwin_art_fdsan_report_error_level();
    if (level == 0) return;
    std::fprintf(stderr, "fdsan: fd %d ownership mismatch: expected 0x%llx, actual 0x%llx\n",
        fd, static_cast<unsigned long long>(expected), static_cast<unsigned long long>(actual));
    if (level == 3) std::abort();
  }
}
}
extern "C" uint64_t darwin_art_bionic_android_fdsan_create_owner_tag(int type, uint64_t tag) {
  if (tag == 0) return 0;
  if ((static_cast<unsigned>(type) & 0xffU) != static_cast<unsigned>(type)) {
    std::fprintf(stderr, "fdsan: invalid owner type: %x\n", static_cast<unsigned>(type));
    std::abort();
  }
  return (static_cast<uint64_t>(static_cast<unsigned>(type)) << 56) |
      (tag & UINT64_C(0x00ffffffffffffff));
}
extern "C" void darwin_art_bionic_android_fdsan_exchange_owner_tag(
    int fd, uint64_t expected, uint64_t next) {
  Exchange(fd, expected, next);
}
extern "C" int darwin_art_bionic_android_fdsan_close_with_tag(int fd, uint64_t tag) {
  if (fd < 0) return darwin_art_bionic_socket_broker_close_unchecked(fd);
  Exchange(fd, tag, 0);
  int result = darwin_art_bionic_socket_broker_close_unchecked(fd);
  if (tag && result == -1 && darwin_art_bionic_errno_load() == 9) {
    const int level = darwin_art_fdsan_report_error_level();
    if (level != 0) std::fprintf(stderr, "fdsan: double-close of tagged fd %d\n", fd);
    if (level == 3) std::abort();
  }
  return result;
}
extern "C" int darwin_art_bionic_socket_broker_close(int fd) {
  const int result = darwin_art_bionic_android_fdsan_close_with_tag(fd, 0);
  // Bionic close hides EINTR; close_with_tag itself preserves the raw result.
  return result == -1 && darwin_art_bionic_errno_load() == 4 ? 0 : result;
}
