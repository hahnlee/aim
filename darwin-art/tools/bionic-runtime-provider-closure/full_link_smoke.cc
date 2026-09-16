#include "darwin_art_bionic_builtin_adapters.h"
#include "darwin_art_bionic_dns.h"
#include "darwin_art_bionic_fs.h"
#include "darwin_art_bionic_socket_broker.h"
#include "darwin_art_bionic_vm.h"

#include <android/log.h>

#include <fcntl.h>
#include <poll.h>
#include <unistd.h>

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>

extern "C" uintptr_t darwin_art_bionic_rust_provider_closure_anchor(void);
extern "C" int darwin_art_ftw_traversal_smoke(void);
extern "C" int darwin_art_unix_connect_smoke(void);
extern "C" int darwin_art_property_client_smoke(void);
extern "C" int darwin_art_property_iteration_smoke(DarwinArtBionicNamespace*);
extern "C" int darwin_art_signed_numeric_smoke(DarwinArtBionicNamespace*);
extern "C" int darwin_art_mkdirat_smoke(DarwinArtBionicNamespace*);
extern "C" int darwin_art_configured_snapshot_smoke(void);
extern "C" int darwin_art_credentials_snapshot_smoke(void);
extern "C" int darwin_art_fortify_stream_smoke(DarwinArtBionicNamespace*);
extern "C" uintptr_t darwin_art_liblog_provider_resolve(const char*, const char*);
extern "C" int darwin_art_test_log_buf_print(uintptr_t, const char*, const char*);
extern "C" int darwin_art_log_assert_smoke();
extern "C" int darwin_art_fdsan_smoke();
extern "C" int darwin_art_bionic_syslog_activate(const char *guest_program_tag);
extern "C" void darwin_art_bionic_syslog(int priority, const char *format, ...);
extern "C" int darwin_art_bionic_stdio_process_install(void);
extern "C" int darwin_art_bionic_stdio_process_uninstall(void);
extern "C" uint32_t darwin_art_bionic_getwc(void *file);
extern "C" uint32_t darwin_art_bionic_ungetwc(uint32_t wc, void *file);
extern "C" unsigned char darwin_art_bionic___sF[456];
extern "C" void *darwin_art_android_ANativeWindow_fromSurface(void *, void *) {
  return nullptr;
}
extern "C" void darwin_art_android_ANativeWindow_release(void *) {}
extern "C" int darwin_art_android_ANativeWindow_lock(void *, void *, void *) {
  return -1;
}
extern "C" int darwin_art_android_ANativeWindow_unlockAndPost(void *) {
  return -1;
}
extern "C" int darwin_art_android_ANativeWindow_setBuffersGeometry(
    void *, int, int, int) {
  return 0;
}
extern "C" int ASharedMemory_create(const char *, size_t) { return -1; }
extern "C" int ASharedMemory_setProt(int, int) { return 0; }
extern "C" void* darwin_art_angle_dso_symbol(const char*, const char*) {
  return nullptr;
}
extern "C" void* darwin_art_android_platform_symbol(const char*) {
  return nullptr;
}

namespace {

struct Expected {
  const char *soname;
  const char *symbol;
  const char *version;
  DarwinArtBionicProviderId owner;
  bool default_version;
};

constexpr Expected kExpected[] = {
#include "ownership.inc"
};

int captured_priority = 0;
char captured_tag[32]{};
char captured_message[32]{};

void Capture(const __android_log_message *message) {
  if (message == nullptr || message->tag == nullptr ||
      message->message == nullptr) {
    return;
  }
  captured_priority = message->priority;
  std::snprintf(captured_tag, sizeof(captured_tag), "%s", message->tag);
  std::snprintf(captured_message, sizeof(captured_message), "%s",
                message->message);
}

} // namespace

int main() {
  if (darwin_art_bionic_rust_provider_closure_anchor() == 0)
    return 10;
  char root_path[] = "/tmp/darwin-art-provider-closure.XXXXXX";
  if (mkdtemp(root_path) == nullptr)
    return 18;
  const int root_fd = open(root_path, O_RDONLY | O_DIRECTORY);
  const int fixture_fd =
      root_fd < 0 ? -1 : openat(root_fd, "fixture", O_CREAT | O_WRONLY, 0600);
  if (fixture_fd < 0 || write(fixture_fd, "x", 1) != 1 ||
      close(fixture_fd) != 0) {
    return 19;
  }
  static constexpr uint8_t kRoot[] = {'/'};
  if (darwin_art_bionic_fs_process_install(root_fd, kRoot, sizeof(kRoot), kRoot,
                                           sizeof(kRoot)) !=
          DARWIN_ART_BIONIC_FS_PROCESS_OWNER_OK ||
      darwin_art_bionic_socket_broker_activate() != 0 ||
      darwin_art_bionic_vm_process_install() != 0) {
    return 20;
  }
  if (darwin_art_ftw_traversal_smoke() != 0) return 21;
  if (darwin_art_unix_connect_smoke() != 0) return 23;
  if (darwin_art_property_client_smoke() != 0) return 24;
  if (darwin_art_configured_snapshot_smoke() != 0) return 27;
  if (darwin_art_credentials_snapshot_smoke() != 0) return 31;
  const int guest_socket = darwin_art_bionic_socket_broker_socket(2, 2, 0);
  int reuse = 1;
  uint32_t reuse_length = sizeof(reuse);
  const int set_option = guest_socket < 0
                             ? -1
                             : darwin_art_bionic_socket_broker_setsockopt(
                                   guest_socket, 1, 2, &reuse, sizeof(reuse));
  reuse = 0;
  const int get_option = set_option != 0
                             ? -1
                             : darwin_art_bionic_socket_broker_getsockopt(
                                   guest_socket, 1, 2, &reuse, &reuse_length);
  const int set_descriptor =
      guest_socket < 0
          ? -1
          : darwin_art_bionic_socket_broker_fcntl(guest_socket, 2, 1);
  const int get_descriptor =
      set_descriptor != 0
          ? -1
          : darwin_art_bionic_socket_broker_fcntl(guest_socket, 1, 0);
  const int close_socket =
      guest_socket < 0 ? -1
                       : darwin_art_bionic_socket_broker_close(guest_socket);
  if (guest_socket < 0 || set_option != 0 || get_option != 0 || reuse != 1 ||
      reuse_length != sizeof(reuse) || set_descriptor != 0 ||
      get_descriptor != 1 || close_socket != 0) {
    std::fprintf(stderr,
                 "bionic-runtime-provider-closure: socket option smoke "
                 "fd=%d set=%d get=%d value=%d length=%u setfd=%d getfd=%d "
                 "close=%d\n",
                 guest_socket, set_option, get_option, reuse, reuse_length,
                 set_descriptor, get_descriptor, close_socket);
    return 23;
  }
  void *guest_mapping =
      darwin_art_bionic_mmap(nullptr, 4096, 0x1 | 0x2, 0x02 | 0x20, -1, 0);
  if (guest_mapping == reinterpret_cast<void *>(UINTPTR_MAX) ||
      darwin_art_bionic_mprotect(guest_mapping, 4096, 0x1) != 0 ||
      darwin_art_bionic_munmap(guest_mapping, 4096) != 0) {
    return 22;
  }
  char guest_program_tag[] = "GuestProgram";
  if (darwin_art_bionic_syslog_activate(guest_program_tag) != 0 ||
      darwin_art_bionic_syslog_activate("SecondProgram") != -1) {
    return 11;
  }
  guest_program_tag[0] = 'X';
  __android_log_set_logger(Capture);
  if (darwin_art_log_assert_smoke() != 0) return 23;
  const auto buffered_log = darwin_art_liblog_provider_resolve(
      "__android_log_buf_print", "LIBLOG");
  if (buffered_log == 0 ||
      darwin_art_test_log_buf_print(buffered_log, "BufABI",
                                   "%d %d %d %d %d %.1f") != 1 ||
      captured_priority != ANDROID_LOG_INFO ||
      std::strcmp(captured_tag, "BufABI") != 0 ||
      std::strcmp(captured_message, "1 2 3 4 5 1.5") != 0) return 22;
  darwin_art_bionic_syslog(6, "owned-copy");
  __android_log_set_logger(__android_log_stderr_logger);
  if (captured_priority != ANDROID_LOG_INFO ||
      std::strcmp(captured_tag, "GuestProgram") != 0 ||
      std::strcmp(captured_message, "owned-copy") != 0) {
    return 12;
  }
  if (darwin_art_bionic_stdio_process_install() != 0 ||
      darwin_art_bionic_ungetwc('Q', darwin_art_bionic___sF) != 'Q' ||
      darwin_art_bionic_getwc(darwin_art_bionic___sF) != 'Q') {
    return 16;
  }
  DarwinArtBionicNamespace *instance = darwin_art_bionic_namespace_create();
  if (instance == nullptr ||
      darwin_art_bionic_namespace_bind_builtins(instance, nullptr) !=
          DARWIN_ART_BIONIC_NAMESPACE_OK ||
      darwin_art_bionic_namespace_seal(instance) !=
          DARWIN_ART_BIONIC_NAMESPACE_OK) {
    return 13;
  }
  for (const Expected &expected : kExpected) {
    const auto result = darwin_art_bionic_namespace_resolve(
        instance, expected.soname, expected.symbol,
        expected.version[0] == '\0' ? nullptr : expected.version);
    if (result.status != DARWIN_ART_BIONIC_NAMESPACE_OK ||
        result.owner != expected.owner || result.address == 0) {
      std::fprintf(stderr,
                   "bionic-runtime-provider-closure: route failed "
                   "%s:%s@%s status=%s owner=%s\n",
                   expected.soname, expected.symbol, expected.version,
                   darwin_art_bionic_namespace_status_name(result.status),
                   darwin_art_bionic_provider_name(result.owner));
      return 14;
    }
  }
  if (darwin_art_fortify_stream_smoke(instance) != 0) return 25;
  if (darwin_art_property_iteration_smoke(instance) != 0) return 28;
  if (darwin_art_signed_numeric_smoke(instance) != 0) return 29;
  if (darwin_art_mkdirat_smoke(instance) != 0) return 30;
  const auto open_route =
      darwin_art_bionic_namespace_resolve(instance, "libc.so", "open", "LIBC");
  const auto close_route =
      darwin_art_bionic_namespace_resolve(instance, "libc.so", "close", "LIBC");
  const auto read_route =
      darwin_art_bionic_namespace_resolve(instance, "libc.so", "read", "LIBC");
  const auto write_route =
      darwin_art_bionic_namespace_resolve(instance, "libc.so", "write", "LIBC");
  const auto pipe_route =
      darwin_art_bionic_namespace_resolve(instance, "libc.so", "pipe", "LIBC");
  const auto poll_route =
      darwin_art_bionic_namespace_resolve(instance, "libc.so", "poll", "LIBC");
  using GuestOpen = int (*)(const char *, int, uint32_t);
  using GuestClose = int (*)(int);
  using GuestRead = intptr_t (*)(int, void *, size_t);
  using GuestWrite = intptr_t (*)(int, const void *, size_t);
  using GuestPipe = int (*)(int32_t *);
  using GuestPoll = int (*)(DarwinArtBionicPollFd *, size_t, int);
  const int guest_fd =
      reinterpret_cast<GuestOpen>(open_route.address)("/fixture", 0, 0);
  char guest_byte = 0;
  if (open_route.owner != DARWIN_ART_BIONIC_PROVIDER_FILESYSTEM ||
      close_route.owner != DARWIN_ART_BIONIC_PROVIDER_CENTRAL_FD_BROKER ||
      read_route.owner != DARWIN_ART_BIONIC_PROVIDER_CENTRAL_FD_BROKER ||
      write_route.owner != DARWIN_ART_BIONIC_PROVIDER_CENTRAL_FD_BROKER ||
      pipe_route.owner != DARWIN_ART_BIONIC_PROVIDER_CENTRAL_FD_BROKER ||
      poll_route.owner != DARWIN_ART_BIONIC_PROVIDER_CENTRAL_FD_BROKER ||
      guest_fd < 0 ||
      reinterpret_cast<GuestRead>(read_route.address)(guest_fd, &guest_byte,
                                                      1) != 1 ||
      guest_byte != 'x' ||
      reinterpret_cast<GuestClose>(close_route.address)(guest_fd) != 0) {
    return 21;
  }
  int32_t pipe_fds[2] = {-1, -1};
  if (darwin_art_fdsan_smoke() != 0) return 26;
  DarwinArtBionicPollFd readable{-1, POLLIN, 0};
  const char sent = 'p';
  char received = 0;
  if (reinterpret_cast<GuestPipe>(pipe_route.address)(pipe_fds) != 0) {
    return 24;
  }
  readable.fd = pipe_fds[0];
  if (reinterpret_cast<GuestPoll>(poll_route.address)(&readable, 1, 0) != 0 ||
      reinterpret_cast<GuestWrite>(write_route.address)(pipe_fds[1], &sent,
                                                        1) != 1 ||
      reinterpret_cast<GuestPoll>(poll_route.address)(&readable, 1, 1000) !=
          1 ||
      (readable.revents & POLLIN) == 0 ||
      reinterpret_cast<GuestRead>(read_route.address)(pipe_fds[0], &received,
                                                      1) != 1 ||
      received != sent ||
      reinterpret_cast<GuestClose>(close_route.address)(pipe_fds[0]) != 0 ||
      reinterpret_cast<GuestClose>(close_route.address)(pipe_fds[1]) != 0) {
    return 25;
  }
  if (darwin_art_bionic_namespace_teardown(instance) !=
      DARWIN_ART_BIONIC_NAMESPACE_OK) {
    return 15;
  }
  darwin_art_bionic_namespace_destroy(instance);
  if (darwin_art_bionic_socket_broker_deactivate() != 0 ||
      darwin_art_bionic_stdio_process_uninstall() != 0 ||
      darwin_art_bionic_vm_process_uninstall() != 0 ||
      darwin_art_bionic_fs_process_uninstall() !=
          DARWIN_ART_BIONIC_FS_PROCESS_OWNER_OK) {
    return 17;
  }
  unlinkat(root_fd, "fixture", 0);
  close(root_fd);
  rmdir(root_path);
  std::fprintf(stderr, "bionic-runtime-provider-closure: PASS bind_builtins=36 "
                       "routes=%zu actual-resolvers=yes fd=fs+pipe-poll "
                       "wide-stdio=central-lease syslog-tag=owned-copy\n",
                       sizeof(kExpected) / sizeof(kExpected[0]));
  return 0;
}
