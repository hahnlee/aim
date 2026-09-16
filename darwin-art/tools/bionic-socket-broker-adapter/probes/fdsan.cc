#include "darwin_art_bionic_socket_broker.h"
#include "../src/fdsan.h"
#include <cassert>
#include <csignal>
#include <cstdio>
#include <sys/resource.h>
#include <sys/wait.h>
#include <unistd.h>
extern "C" int darwin_art_bionic_fs_open_core(const char*, int, uint32_t);
extern "C" int darwin_art_bionic_fs_fcntl_core(int, int, intptr_t);

extern "C" int darwin_art_fdsan_smoke() {
  // Uses the real process property provider in the linked closure, not a spy.
  assert(darwin_art_bionic_android_fdsan_set_error_level_from_property(3) == 3);
  assert(darwin_art_fdsan_get_error_level() == 3);
  // Exercise the actual public resolver, not a test-only symbol table.
  const auto get_level = reinterpret_cast<int (*)()>(
      darwin_art_bionic_socket_broker_resolve(
          "libc.so", "android_fdsan_get_error_level", "LIBC_Q"));
  const auto set_level = reinterpret_cast<int (*)(int)>(
      darwin_art_bionic_socket_broker_resolve(
          "libc.so", "android_fdsan_set_error_level", "LIBC_Q"));
  assert(get_level != nullptr && set_level != nullptr);
  const auto get_tag = reinterpret_cast<uint64_t (*)(int)>(
      darwin_art_bionic_socket_broker_resolve(
          "libc.so", "android_fdsan_get_owner_tag", "LIBC_Q"));
  assert(get_tag && get_tag(-1) == 0);
  assert(!darwin_art_bionic_socket_broker_resolve(
      "libc.so", "android_fdsan_get_owner_tag", "LIBC"));
  assert(get_level() == 3);
  assert(set_level(2) == 3 && get_level() == 2);
  assert(set_level(3) == 2 && get_level() == 3);
  assert(darwin_art_bionic_socket_broker_resolve(
      "libc.so", "android_fdsan_get_error_level", "LIBC") == nullptr);
  assert(darwin_art_bionic_socket_broker_resolve(
      "libdl.so", "android_fdsan_get_error_level", "LIBC_Q") == nullptr);
  assert(darwin_art_bionic_socket_broker_resolve(
      "libc.so", "android_fdsan_set_error_level_from_property", "LIBC_Q") == nullptr);
  int32_t fds[2];
  assert(darwin_art_bionic_socket_broker_pipe(fds) == 0);
  const uint64_t tag = darwin_art_bionic_android_fdsan_create_owner_tag(1, 1234);
  assert(darwin_art_bionic_android_fdsan_close_with_tag(-1, tag) == -1);
  darwin_art_bionic_android_fdsan_exchange_owner_tag(fds[0], 0, tag);
  const int duplicate = darwin_art_bionic_socket_broker_dup(fds[0]);
  assert(get_tag(fds[0]) == tag);
  assert(get_tag(duplicate) == 0);
  assert(duplicate >= 0);
  // Ownership is attached to descriptor, not the shared open-file description.
  assert(darwin_art_bionic_socket_broker_close(duplicate) == 0);
  for (int kind = 0; kind != 2; ++kind) {
    const pid_t child = fork();
    assert(child >= 0);
    if (!child) {
      const rlimit no_core{0, 0};
      if (setrlimit(RLIMIT_CORE, &no_core)) _exit(90);
      if (kind == 0) darwin_art_bionic_socket_broker_close(fds[0]);
      else darwin_art_bionic_android_fdsan_exchange_owner_tag(fds[0], tag + 1, 0);
      _exit(91);
    }
    int status = 0;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFSIGNALED(status) && WTERMSIG(status) == SIGABRT);
  }
  assert(darwin_art_bionic_android_fdsan_close_with_tag(fds[0], tag) == 0);
  assert(get_tag(fds[0]) == 0);
  assert(darwin_art_bionic_socket_broker_close(fds[1]) == 0);
  const int file = darwin_art_bionic_fs_open_core("/fixture", 0, 0);
  assert(file >= 0);
  darwin_art_bionic_android_fdsan_exchange_owner_tag(file, 0, tag);
  // dup clears FD_CLOEXEC on the new descriptor, without changing the source
  // flags or inheriting its ownership tag (Android F_GETFD=1/F_SETFD=2).
  assert(darwin_art_bionic_fs_fcntl_core(file, 2, 1) == 0);
  const int file_duplicate = darwin_art_bionic_socket_broker_dup(file);
  assert(file_duplicate >= 0 && file_duplicate != file);
  assert(darwin_art_bionic_fs_fcntl_core(file, 1, 0) == 1);
  assert(darwin_art_bionic_fs_fcntl_core(file_duplicate, 1, 0) == 0);
  assert(darwin_art_bionic_socket_broker_close(file_duplicate) == 0);
  assert(darwin_art_bionic_android_fdsan_close_with_tag(file, tag) == 0);
  std::puts("fdsan: pipe/file dup unowned, dup clears CLOEXEC, wrong exchange/plain close fatal, tagged close PASS");
  return 0;
}
