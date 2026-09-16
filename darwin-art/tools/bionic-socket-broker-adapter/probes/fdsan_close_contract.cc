// Unit boundary: controlled raw-close failure, real FDSAN tag ownership module.
#include "../src/fdsan.h"
#include "darwin_art_bionic_socket_broker.h"
#include <cassert>
#include <csignal>
#include <cstdio>
#include <initializer_list>
#include <cstring>
#include <sys/resource.h>
#include <sys/wait.h>
#include <unistd.h>

static int android_error;
static int raw_result;
static int raw_calls;
static const char* property_value;
extern "C" const void* darwin_art_bionic___system_property_find(const char* name) {
  assert(std::strcmp(name, "debug.fdsan") == 0);
  return property_value;
}
extern "C" void darwin_art_bionic___system_property_read_callback(
    const void* property, void (*callback)(void*, const char*, const char*, uint32_t), void* cookie) {
  assert(property == property_value);
  callback(cookie, "debug.fdsan", property_value, 1);
}
extern "C" int darwin_art_bionic_errno_load() { return android_error; }
extern "C" int darwin_art_bionic_socket_broker_close_unchecked(int) { ++raw_calls; return raw_result; }

int main() {
  assert(darwin_art_bionic_android_fdsan_create_owner_tag(1, 0) == 0);
  assert(darwin_art_bionic_android_fdsan_create_owner_tag(-1, 0) == 0);
  assert(darwin_art_bionic_android_fdsan_create_owner_tag(255, UINT64_MAX) == UINT64_MAX);
  for (int type : {-1, 256}) {
    const pid_t child = fork();
    assert(child >= 0);
    if (!child) {
      const rlimit no_core{0, 0};
      if (setrlimit(RLIMIT_CORE, &no_core)) _exit(90);
      darwin_art_bionic_android_fdsan_create_owner_tag(type, 1);
      _exit(91);
    }
    int status = 0;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFSIGNALED(status) && WTERMSIG(status) == SIGABRT);
  }
  darwin_art_fdsan_initialize();
  raw_result = -1;
  android_error = 4;
  assert(darwin_art_bionic_socket_broker_close(100) == 0);
  assert(android_error == 4);
  assert(darwin_art_bionic_android_fdsan_close_with_tag(101, 0) == -1);
  android_error = 9;
  assert(darwin_art_bionic_socket_broker_close(102) == -1);
  raw_result = 0;
  assert(darwin_art_bionic_socket_broker_close(103) == 0);
  assert(darwin_art_fdsan_get_error_level() == 3);
  darwin_art_bionic_android_fdsan_exchange_owner_tag(200, 0, 42);
  assert(darwin_art_fdsan_set_error_level(1) == 3);
  darwin_art_bionic_android_fdsan_exchange_owner_tag(200, 43, 0);
  assert(darwin_art_fdsan_get_error_level() == 0);
  // Failed exchange did not replace the actual owner's tag.
  darwin_art_bionic_android_fdsan_exchange_owner_tag(200, 42, 0);
  assert(darwin_art_fdsan_set_error_level(2) == 0);
  darwin_art_bionic_android_fdsan_exchange_owner_tag(201, 0, 42);
  const int calls_before = raw_calls;
  assert(darwin_art_bionic_socket_broker_close(201) == 0);
  assert(raw_calls == calls_before + 1 && darwin_art_fdsan_get_error_level() == 2);
  darwin_art_bionic_android_fdsan_exchange_owner_tag(201, 42, 0);
  raw_result = -1;
  android_error = 9;
  // WARN_ONCE double-close is nonfatal and preserves raw EBADF.
  assert(darwin_art_fdsan_set_error_level(1) == 2);
  darwin_art_bionic_android_fdsan_exchange_owner_tag(202, 0, 42);
  assert(darwin_art_bionic_android_fdsan_close_with_tag(202, 42) == -1);
  assert(android_error == 9 && darwin_art_fdsan_get_error_level() == 0);
  assert(darwin_art_fdsan_set_error_level(3) == 0);
  assert(darwin_art_bionic_android_fdsan_set_error_level_from_property(1) == 3);
  assert(darwin_art_fdsan_get_error_level() == 1);
  struct Case { const char* text; int expected; };
  for (const auto& test : {Case{"1", 3}, Case{"FaTaL", 3}, Case{"WaRn", 2},
                           Case{"warn_once", 1}, Case{"", 2}, Case{"0", 2}, Case{"unknown", 2}}) {
    property_value = test.text;
    darwin_art_fdsan_set_error_level(0);
    assert(darwin_art_bionic_android_fdsan_set_error_level_from_property(2) == 0);
    assert(darwin_art_fdsan_get_error_level() == test.expected);
  }
  property_value = nullptr;
  darwin_art_fdsan_set_error_level(3);
  std::puts("FDSAN property: absent/default, case-insensitive values, zero/unknown fallback, previous level PASS");
  std::puts("FDSAN levels: warn-once disables, warn-always continues close, double-close severity PASS");
  std::puts("FDSAN tag construction and close/close_with_tag EINTR distinction PASS");
}
