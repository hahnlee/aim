#include "../../../compat/surfaceflinger/sync/sync.h"
#include "darwin_art_bionic_socket_broker.h"

#include <csignal>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <thread>

extern "C" DarwinArtBionicSocketBrokerFunction
darwin_art_bionic_socket_broker_fdsan_resolve(const char *, const char *,
                                              const char *) {
  // The focused test does not exercise symbol lookup; the real fdsan close
  // implementation remains linked below. Keep this unrelated resolver seam
  // inert so an old prebuilt owner library cannot change merge coverage.
  return nullptr;
}

// The focused executable exercises the real socket-broker/pipe/ioctl/poll
// path. These unrelated filesystem symbols are the same inert test boundary
// used by the adapter's existing runner; no merge operation reaches them.
extern "C" int darwin_art_bionic_fs_close_core(int) { return -1; }
extern "C" intptr_t darwin_art_bionic_fs_read_core(int, void *, size_t) {
  return -1;
}
extern "C" intptr_t darwin_art_bionic_fs_write_core(int, const void *, size_t) {
  return -1;
}
extern "C" int darwin_art_bionic_fs_fcntl_core(int, int, intptr_t) {
  return -1;
}
extern "C" int darwin_art_bionic_fs_dup_host_fd_core(int, int *) { return 0; }
extern "C" int darwin_art_bionic_fs_adopt_host_fd_core(int) { return -1; }

namespace {
constexpr uint32_t kSyncIocMerge = UINT32_C(0xc0303e03);
constexpr uint32_t kSyncIocFileInfo = UINT32_C(0xc0383e04);

struct AndroidSyncMergeData {
  char name[32];
  int32_t fd2;
  int32_t fence;
  uint32_t flags;
  uint32_t pad;
};

struct AndroidSyncFileInfo {
  char name[32];
  int32_t status;
  uint32_t flags;
  uint32_t num_fences;
  uint32_t pad;
  uint64_t sync_fence_info;
};

bool Fail(const char *message) {
  std::fprintf(stderr, "sync-fence-merge: FAIL %s\n", message);
  return false;
}

bool Check(bool condition, const char *message) {
  return condition ? true : Fail(message);
}

void CloseIfOpen(int *fd) {
  if (*fd >= 0) {
    (void)darwin_art_bionic_socket_broker_close(*fd);
    *fd = -1;
  }
}

bool Signal(int fd) {
  const uint64_t marker = 1;
  return darwin_art_bionic_socket_broker_write(fd, &marker, sizeof(marker)) ==
         static_cast<intptr_t>(sizeof(marker));
}

int PollEvents(int fd, int timeout_ms, int16_t *revents) {
  DarwinArtBionicPollFd descriptor{fd, 0x0001, 0};
  const int result =
      darwin_art_bionic_socket_broker_poll(&descriptor, 1, timeout_ms);
  if (revents != nullptr)
    *revents = descriptor.revents;
  return result;
}

bool IsReady(int fd, int timeout_ms) {
  int16_t revents = 0;
  return PollEvents(fd, timeout_ms, &revents) == 1 && (revents & 0x0001) != 0;
}

bool Merge(int fd1, int fd2, int *merged) {
  AndroidSyncMergeData data{};
  std::memcpy(data.name, "merge-test", sizeof("merge-test") - 1);
  data.fd2 = fd2;
  data.fence = -1;
  int handled = 0;
  int result = -1;
  int android_errno = 0;
  if (darwin_art_bionic_socket_broker_ioctl_dispatch(
          fd1, kSyncIocMerge, &data, &handled, &result, &android_errno) != 0)
    return Fail("ioctl dispatch failed");
  if (!Check(handled == 1 && result == 0 && android_errno == 0,
             "merge ioctl was not handled successfully"))
    return false;
  if (!Check(data.fence >= 0, "merge ioctl did not return a fence"))
    return false;
  *merged = data.fence;
  return true;
}

bool RunDependencyTest() {
  int first[2] = {-1, -1};
  int second[2] = {-1, -1};
  int merged = -1;
  bool success = false;
  if (darwin_art_bionic_socket_broker_pipe(first) != 0 ||
      darwin_art_bionic_socket_broker_pipe(second) != 0) {
    Fail("pipe creation failed");
    goto cleanup;
  }
  if (!Merge(first[0], second[0], &merged))
    goto cleanup;
  if (!Check(!IsReady(merged, 0), "merged fence was initially signaled"))
    goto cleanup;
  if (!Check(Signal(second[1]), "failed to signal second fence"))
    goto cleanup;
  if (!Check(IsReady(second[0], 0), "reading merged input consumed fd2 marker"))
    goto cleanup;
  if (!Check(!IsReady(merged, 20),
             "signaling only fd2 incorrectly signaled merged fence"))
    goto cleanup;
  if (!Check(Signal(first[1]), "failed to signal first fence"))
    goto cleanup;
  if (!Check(IsReady(first[0], 0), "reading merged input consumed fd1 marker"))
    goto cleanup;
  if (!Check(IsReady(merged, 1000),
             "merged fence did not signal after both inputs"))
    goto cleanup;
  if (!Check(sync_wait(merged, 1000) == 0,
             "sync_wait rejected a completed merged fence"))
    goto cleanup;
  success = true;

cleanup:
  CloseIfOpen(&merged);
  CloseIfOpen(&second[1]);
  CloseIfOpen(&second[0]);
  CloseIfOpen(&first[1]);
  CloseIfOpen(&first[0]);
  return success;
}

bool RunInvalidSecondTest() {
  int first[2] = {-1, -1};
  int merged = -1;
  AndroidSyncMergeData data{};
  int handled = 0;
  int result = -1;
  int android_errno = 0;
  bool success = false;
  if (darwin_art_bionic_socket_broker_pipe(first) != 0) {
    Fail("invalid-second pipe creation failed");
    goto cleanup;
  }
  data.fd2 = -1;
  data.fence = -1;
  if (darwin_art_bionic_socket_broker_ioctl_dispatch(first[0], kSyncIocMerge,
                                                     &data, &handled, &result,
                                                     &android_errno) != 0) {
    Fail("invalid-second ioctl dispatch failed");
    goto cleanup;
  }
  success = Check(handled == 1 && result == -1 && android_errno != 0 &&
                      data.fence == -1,
                  "invalid second input was accepted");

cleanup:
  CloseIfOpen(&merged);
  CloseIfOpen(&first[1]);
  CloseIfOpen(&first[0]);
  return success;
}

bool RunSameFdTest() {
  int source[2] = {-1, -1};
  int merged = -1;
  bool success = false;
  if (darwin_art_bionic_socket_broker_pipe(source) != 0) {
    Fail("same-fd pipe creation failed");
    goto cleanup;
  }
  if (!Merge(source[0], source[0], &merged))
    goto cleanup;
  CloseIfOpen(&source[0]);
  if (!Check(Signal(source[1]), "failed to signal same-fd input") ||
      !Check(IsReady(merged, 1000),
             "same-fd merge did not complete from one marker"))
    goto cleanup;
  success = true;

cleanup:
  CloseIfOpen(&merged);
  CloseIfOpen(&source[1]);
  CloseIfOpen(&source[0]);
  return success;
}

bool RunOutputCloseTest() {
  int first[2] = {-1, -1};
  int second[2] = {-1, -1};
  int merged = -1;
  bool success = false;
  if (darwin_art_bionic_socket_broker_pipe(first) != 0 ||
      darwin_art_bionic_socket_broker_pipe(second) != 0) {
    Fail("output-close pipe creation failed");
    goto cleanup;
  }
  if (!Merge(first[0], second[0], &merged))
    goto cleanup;
  CloseIfOpen(&merged);
  if (!Check(Signal(first[1]), "failed to signal cancelled first input") ||
      !Check(Signal(second[1]), "failed to signal cancelled second input"))
    goto cleanup;
  success = true;

cleanup:
  CloseIfOpen(&merged);
  CloseIfOpen(&second[1]);
  CloseIfOpen(&second[0]);
  CloseIfOpen(&first[1]);
  CloseIfOpen(&first[0]);
  return success;
}

bool RunOriginalCloseTest() {
  int first[2] = {-1, -1};
  int second[2] = {-1, -1};
  int merged = -1;
  bool success = false;
  if (darwin_art_bionic_socket_broker_pipe(first) != 0 ||
      darwin_art_bionic_socket_broker_pipe(second) != 0) {
    Fail("reverse-order pipe creation failed");
    goto cleanup;
  }
  if (!Merge(first[0], second[0], &merged))
    goto cleanup;

  // The merged fence must retain both inputs after their original guest
  // descriptors are closed. Close the second input first to exercise reverse
  // ordering, then the first input.
  CloseIfOpen(&second[0]);
  CloseIfOpen(&first[0]);
  if (!Check(Signal(first[1]),
             "first input was not retained after closing originals"))
    goto cleanup;
  if (!Check(!IsReady(merged, 20),
             "merged fence signaled with only retained fd1 complete"))
    goto cleanup;
  if (!Check(Signal(second[1]), "failed to signal retained second input"))
    goto cleanup;
  if (!Check(IsReady(merged, 1000), "merged fence lost an input after close"))
    goto cleanup;
  success = true;

cleanup:
  CloseIfOpen(&merged);
  CloseIfOpen(&second[1]);
  CloseIfOpen(&second[0]);
  CloseIfOpen(&first[1]);
  CloseIfOpen(&first[0]);
  return success;
}

bool RunNestedMergeTest() {
  int first[2] = {-1, -1};
  int second[2] = {-1, -1};
  int third[2] = {-1, -1};
  int first_merge = -1;
  int nested_merge = -1;
  bool success = false;
  if (darwin_art_bionic_socket_broker_pipe(first) != 0 ||
      darwin_art_bionic_socket_broker_pipe(second) != 0 ||
      darwin_art_bionic_socket_broker_pipe(third) != 0) {
    Fail("nested-merge pipe creation failed");
    goto cleanup;
  }
  if (!Merge(first[0], second[0], &first_merge) ||
      !Merge(first_merge, third[0], &nested_merge))
    goto cleanup;

  // A nested merge owns independent references to all dependencies. Closing
  // A, B, and the intermediate AB guest read descriptors must not cancel the
  // final ABC fence.
  CloseIfOpen(&first[0]);
  CloseIfOpen(&second[0]);
  CloseIfOpen(&first_merge);
  if (!Check(Signal(second[1]), "failed to signal nested B fence") ||
      !Check(Signal(third[1]), "failed to signal nested C fence"))
    goto cleanup;
  if (!Check(!IsReady(nested_merge, 20),
             "nested merge signaled before dependency A completed"))
    goto cleanup;
  if (!Check(Signal(first[1]), "failed to signal nested A fence"))
    goto cleanup;
  if (!Check(IsReady(nested_merge, 1000),
             "nested merge did not signal after all dependencies"))
    goto cleanup;
  success = true;

cleanup:
  CloseIfOpen(&nested_merge);
  CloseIfOpen(&third[1]);
  CloseIfOpen(&third[0]);
  CloseIfOpen(&second[1]);
  CloseIfOpen(&second[0]);
  CloseIfOpen(&first[1]);
  CloseIfOpen(&first[0]);
  CloseIfOpen(&first_merge);
  return success;
}

bool RunEofBeforeMarkerTest() {
  int first[2] = {-1, -1};
  int second[2] = {-1, -1};
  int merged = -1;
  AndroidSyncFileInfo info{};
  int handled = 0;
  int result = -1;
  int android_errno = 0;
  int16_t revents = 0;
  bool success = false;
  if (darwin_art_bionic_socket_broker_pipe(first) != 0 ||
      darwin_art_bionic_socket_broker_pipe(second) != 0) {
    Fail("EOF-before-marker pipe creation failed");
    goto cleanup;
  }
  if (!Merge(first[0], second[0], &merged))
    goto cleanup;
  // EOF on one dependency is an error, not a successful fence marker.
  CloseIfOpen(&first[1]);
  if (!Check(PollEvents(merged, 20, &revents) == 0,
             "EOF dependency completed output before other input ended"))
    goto cleanup;
  if (!Check(Signal(second[1]), "failed to signal EOF test second input"))
    goto cleanup;
  if (!Check(PollEvents(merged, 1000, &revents) >= 0,
             "poll failed after EOF dependency"))
    goto cleanup;
  // Darwin may report POLLIN together with POLLHUP for an empty pipe. Only a
  // bare POLLIN is a successful readable marker; HUP is an error completion.
  if (!Check((revents & 0x0001) == 0 || (revents & 0x0010) != 0,
             "EOF dependency produced a successful readable marker"))
    goto cleanup;

  info.sync_fence_info = 0;
  if (!Check(darwin_art_bionic_socket_broker_ioctl_dispatch(
                 merged, kSyncIocFileInfo, &info, &handled, &result,
                 &android_errno) == 0 &&
                 handled == 1 && result == 0 && android_errno == 0 &&
                 info.status < 0,
             "EOF fence info did not report a negative completion error"))
    goto cleanup;
  if (!Check(sync_wait(merged, 0) < 0,
             "sync_wait accepted EOF/HUP as successful completion"))
    goto cleanup;
  success = true;

cleanup:
  CloseIfOpen(&merged);
  CloseIfOpen(&second[1]);
  CloseIfOpen(&second[0]);
  CloseIfOpen(&first[1]);
  CloseIfOpen(&first[0]);
  return success;
}

bool RunCompletionCloseStress() {
  for (int iteration = 0; iteration != 100; ++iteration) {
    int first[2] = {-1, -1};
    int second[2] = {-1, -1};
    int merged = -1;
    if (darwin_art_bionic_socket_broker_pipe(first) != 0 ||
        darwin_art_bionic_socket_broker_pipe(second) != 0) {
      CloseIfOpen(&second[1]);
      CloseIfOpen(&second[0]);
      CloseIfOpen(&first[1]);
      CloseIfOpen(&first[0]);
      return Fail("completion/close stress pipe creation failed");
    }
    if (!Merge(first[0], second[0], &merged)) {
      CloseIfOpen(&second[1]);
      CloseIfOpen(&second[0]);
      CloseIfOpen(&first[1]);
      CloseIfOpen(&first[0]);
      return false;
    }

    std::thread signaler([&] {
      (void)Signal(first[1]);
      (void)Signal(second[1]);
    });
    if ((iteration & 1) == 0) {
      CloseIfOpen(&merged);
    } else {
      int16_t ignored_revents = 0;
      (void)PollEvents(merged, 1000, &ignored_revents);
      CloseIfOpen(&merged);
    }
    signaler.join();
    CloseIfOpen(&second[1]);
    CloseIfOpen(&second[0]);
    CloseIfOpen(&first[1]);
    CloseIfOpen(&first[0]);
    if (!Check(darwin_art_bionic_socket_broker_live_objects() == 0,
               "completion/close stress leaked broker objects"))
      return false;
  }
  return true;
}
} // namespace

int main() {
  std::signal(SIGPIPE, SIG_IGN);
  if (!Check(darwin_art_bionic_socket_broker_activate() == 0,
             "broker activation failed"))
    return 1;
  const bool invalid_second = RunInvalidSecondTest();
  const bool dependency = invalid_second && RunDependencyTest();
  const bool same_fd = dependency && RunSameFdTest();
  const bool originals = same_fd && RunOriginalCloseTest();
  const bool output_close = originals && RunOutputCloseTest();
  const bool nested = output_close && RunNestedMergeTest();
  const bool eof = RunEofBeforeMarkerTest();
  const bool stress = RunCompletionCloseStress();
  const bool no_live_objects =
      Check(darwin_art_bionic_socket_broker_live_objects() == 0,
            "broker objects remain before immediate deactivate");
  const int deactivated = darwin_art_bionic_socket_broker_deactivate();
  if (!invalid_second || !dependency || !same_fd || !originals ||
      !output_close || !nested || !eof || !stress || !no_live_objects ||
      !Check(deactivated == 0, "broker deactivation failed"))
    return 1;
  std::puts("sync-fence-merge: PASS two-input readiness reverse-close");
  return 0;
}
