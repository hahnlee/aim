#pragma once
// Test-only substitute for the Rust descriptor table, never a runtime input.
#include <cerrno>
#include <cstdint>
#include <map>
#include <unistd.h>
static std::map<int, int> table;
static int next_descriptor = 100000;
static bool fail_publication = false;
extern "C" int darwin_art_bionic_fs_adopt_host_fd_core(int fd) {
  if (fail_publication) { close(fd); errno = EMFILE; return -1; }
  int guest = next_descriptor++;
  table.emplace(guest, fd);
  return guest;
}
extern "C" int darwin_art_bionic_fs_dup_host_fd_core(int guest, int* host) {
  auto found = table.find(guest);
  if (found == table.end()) return 0;
  *host = dup(found->second);
  return *host < 0 ? -1 : 1;
}
extern "C" int darwin_art_bionic_fs_close_core(int guest) {
  auto found = table.find(guest);
  if (found == table.end()) { errno = EBADF; return -1; }
  int result = close(found->second);
  table.erase(found);
  return result;
}
extern "C" int darwin_art_bionic_errno_load() { return errno; }
extern "C" int darwin_art_bionic_errno_from_darwin(int host, int32_t* guest) {
  *guest = host;
  return 1;
}
