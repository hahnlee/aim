#include "fd_inheritance.h"

#include <atomic>
#include <cerrno>
#include <fcntl.h>
#include <sys/socket.h>
#include <unistd.h>

namespace darwin_art::bionic::fd_inheritance {
namespace {

std::atomic<FdInheritanceBoundary> g_boundary{nullptr};

bool SetCloseOnExec(int descriptor) noexcept {
  int flags;
  do {
    flags = fcntl(descriptor, F_GETFD);
  } while (flags < 0 && errno == EINTR);
  if (flags < 0)
    return false;

  int result;
  do {
    result = fcntl(descriptor, F_SETFD, flags | FD_CLOEXEC);
  } while (result < 0 && errno == EINTR);
  return result == 0;
}

void ClosePair(int descriptors[2], int saved_errno) noexcept {
  if (descriptors[0] >= 0)
    close(descriptors[0]);
  if (descriptors[1] >= 0)
    close(descriptors[1]);
  descriptors[0] = -1;
  descriptors[1] = -1;
  errno = saved_errno;
}

struct SocketContext {
  int domain;
  int type;
  int protocol;
};

intptr_t CreateSocketOperation(void *opaque) noexcept {
  const auto *context = static_cast<const SocketContext *>(opaque);
  const int descriptor =
      socket(context->domain, context->type, context->protocol);
  if (descriptor < 0)
    return -1;
  if (SetCloseOnExec(descriptor))
    return descriptor;

  const int saved_errno = errno;
  close(descriptor);
  errno = saved_errno;
  return -1;
}

struct PairContext {
  int domain;
  int type;
  int protocol;
  int *descriptors;
};

intptr_t CreateSocketPairOperation(void *opaque) noexcept {
  auto *context = static_cast<PairContext *>(opaque);
  int *descriptors = context->descriptors;
  if (socketpair(context->domain, context->type, context->protocol,
                 descriptors) < 0) {
    descriptors[0] = -1;
    descriptors[1] = -1;
    return -1;
  }

  if (SetCloseOnExec(descriptors[0]) && SetCloseOnExec(descriptors[1])) {
    return 0;
  }

  const int saved_errno = errno;
  ClosePair(descriptors, saved_errno);
  return -1;
}

struct PipeContext {
  int *descriptors;
};

intptr_t CreatePipeOperation(void *opaque) noexcept {
  auto *context = static_cast<PipeContext *>(opaque);
  int *descriptors = context->descriptors;
  if (pipe(descriptors) < 0) {
    descriptors[0] = -1;
    descriptors[1] = -1;
    return -1;
  }

  if (SetCloseOnExec(descriptors[0]) && SetCloseOnExec(descriptors[1])) {
    return 0;
  }

  const int saved_errno = errno;
  ClosePair(descriptors, saved_errno);
  return -1;
}

} // namespace

intptr_t RunFdOperation(FdOperation operation, void *context) noexcept {
  if (operation == nullptr) {
    errno = EINVAL;
    return -1;
  }
  const FdInheritanceBoundary boundary =
      g_boundary.load(std::memory_order_acquire);
  if (boundary == nullptr) {
    errno = ENOSYS;
    return -1;
  }
  // Do not touch errno after returning from the trusted boundary: the Rust
  // callback owns preservation of the operation's native errno/result pair.
  return boundary(operation, context);
}

int CreateCloseOnExecSocket(int domain, int type, int protocol) noexcept {
  SocketContext context{domain, type, protocol};
  return static_cast<int>(RunFdOperation(&CreateSocketOperation, &context));
}

int CreateCloseOnExecSocketPair(int domain, int type, int protocol,
                                int descriptors[2]) noexcept {
  if (descriptors == nullptr) {
    errno = EINVAL;
    return -1;
  }
  descriptors[0] = -1;
  descriptors[1] = -1;
  PairContext context{domain, type, protocol, descriptors};
  return static_cast<int>(RunFdOperation(&CreateSocketPairOperation, &context));
}

int CreateCloseOnExecPipe(int descriptors[2]) noexcept {
  if (descriptors == nullptr) {
    errno = EINVAL;
    return -1;
  }
  descriptors[0] = -1;
  descriptors[1] = -1;
  PipeContext context{descriptors};
  return static_cast<int>(RunFdOperation(&CreatePipeOperation, &context));
}

} // namespace darwin_art::bionic::fd_inheritance

extern "C" int darwin_art_bionic_install_fd_inheritance_boundary(
    darwin_art::bionic::fd_inheritance::FdInheritanceBoundary boundary) {
  if (boundary == nullptr) {
    errno = EINVAL;
    return -1;
  }

  auto installed = darwin_art::bionic::fd_inheritance::g_boundary.load(
      std::memory_order_acquire);
  if (installed == boundary)
    return 0;
  if (installed != nullptr) {
    errno = EBUSY;
    return -1;
  }

  if (darwin_art::bionic::fd_inheritance::g_boundary.compare_exchange_strong(
          installed, boundary, std::memory_order_release,
          std::memory_order_acquire)) {
    return 0;
  }
  if (installed == boundary)
    return 0;
  errno = EBUSY;
  return -1;
}
