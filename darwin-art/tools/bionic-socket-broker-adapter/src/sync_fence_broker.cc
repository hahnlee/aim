#include "sync_fence_broker.h"
#include "fd_inheritance.h"
#include <cerrno>
#include <fcntl.h>
#include <sys/stat.h>
#include <unistd.h>
#include <utility>

namespace darwin_art::socket {
namespace {
struct HostFd {
  int value = -1;
  ~HostFd() { if (value >= 0) close(value); }
  int take() { return std::exchange(value, -1); }
};
struct Inputs {
  explicit Inputs(DarwinArtFdBroker* value) : broker(value) {}
  DarwinArtFdBroker* broker;
  int descriptors[2] = {-1, -1};
  ~Inputs() {
    for (int fd : descriptors) {
      if (fd < 0) continue;
      DarwinArtFdIoResult ignored{};
      (void)darwin_art_fd_broker_close(broker, fd, &ignored);
    }
  }
};
int BrokerError(DarwinArtFdBrokerStatus status) {
  switch (status) {
    case DARWIN_ART_FD_BROKER_EXHAUSTED: return 24;
    case DARWIN_ART_FD_BROKER_DRAINING: return 16;
    case DARWIN_ART_FD_BROKER_UNSUPPORTED: return 95;
    default: return 9;
  }
}
bool ReadPipe(int fd) {
  struct stat info{};
  const int flags = fcntl(fd, F_GETFL);
  return flags >= 0 && (flags & O_ACCMODE) == O_RDONLY &&
         fstat(fd, &info) == 0 && S_ISFIFO(info.st_mode);
}
}

int MergeBrokerFences(DarwinArtFdBroker* broker, int first, int second,
    void* context, PublishSyncFence publish, int* error) noexcept {
  if (!error) return -1;
  *error = 22;
  if (!broker || !publish) return -1;
  try {
    auto inputs = std::make_shared<Inputs>(broker);
    const int originals[2] = {first, second};
    HostFd native[2];
    for (int i = 0; i < 2; ++i) {
      auto status = darwin_art_fd_broker_dup(broker, originals[i], &inputs->descriptors[i]);
      if (status != DARWIN_ART_FD_BROKER_OK) {
        *error = BrokerError(status); return -1;
      }
      DarwinArtFdIoResult result{};
      status = darwin_art_fd_broker_export_host_fd(
          broker, inputs->descriptors[i], &native[i].value, &result);
      if (status != DARWIN_ART_FD_BROKER_OK) {
        *error = result.android_errno ? result.android_errno : BrokerError(status);
        return -1;
      }
      // This Darwin transport supports native pipe-backed fences, including
      // Metal completion and recursively merged fences. Other exported objects
      // are not silently treated as a completed sync_file.
      if (!ReadPipe(native[i].value)) { *error = 22; return -1; }
    }
    int pair[2];
    if (darwin_art::bionic::fd_inheritance::CreateCloseOnExecPipe(pair) != 0) {
      *error = errno == EMFILE ? 24 : 12; return -1;
    }
    HostFd read{pair[0]}, write{pair[1]};
    if (fcntl(read.value, F_SETFD, FD_CLOEXEC) != 0 ||
        fcntl(write.value, F_SETFD, FD_CLOEXEC) != 0) {
      *error = 5; return -1;
    }
    // Keep broker descriptions as well as native dup FDs alive. A nested merge
    // must not cancel its input owner merely because the caller closes its FD.
    auto merge = SyncFenceMerge::Create(
        native[0].take(), native[1].take(), write.take(), std::move(inputs));
    if (!merge) { *error = 12; return -1; }
    return publish(context, read.take(), std::move(merge), error);
  } catch (const std::bad_alloc&) {
    *error = 12; return -1;
  }
}
}
