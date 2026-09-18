#include "tools/bionic-socket-broker-adapter/src/fd_inheritance.h"

#include <assert.h>
#include <cerrno>
#include <fcntl.h>
#include <stdint.h>
#include <string.h>
#include <unistd.h>

#include <sys/socket.h>

namespace inheritance = darwin_art::bionic::fd_inheritance;

namespace {

int boundary_calls = 0;
inheritance::FdOperation observed_operation = nullptr;
void* observed_context = nullptr;

intptr_t ForwardForTest(inheritance::FdOperation operation,
                        void* context) {
  ++boundary_calls;
  observed_operation = operation;
  observed_context = context;
  return operation(context);
}

intptr_t DifferentBoundaryForTest(inheritance::FdOperation, void*) {
  return -1;
}

intptr_t NeverCalled(void*) {
  assert(false && "missing boundary must fail closed before invoking operation");
  return 7;
}

intptr_t ResultAndErrnoForTest(void*) {
  errno = ERANGE;
  return 123;
}

void AssertCloseOnExec(int descriptor) {
  const int flags = fcntl(descriptor, F_GETFD);
  assert(flags >= 0);
  assert((flags & FD_CLOEXEC) != 0);
}

void ClosePair(int descriptors[2]) {
  assert(close(descriptors[0]) == 0);
  assert(close(descriptors[1]) == 0);
  descriptors[0] = -1;
  descriptors[1] = -1;
}

}  // namespace

int main() {
  // This is a standalone native-port test.  Its forwarding callback is
  // TESTONLY; it does not claim to be the Rust inheritance mutex or spawn
  // closure integration.
  errno = 0;
  assert(inheritance::RunFdOperation(&NeverCalled, nullptr) == -1);
  assert(errno == ENOSYS);

  errno = 0;
  assert(inheritance::CreateCloseOnExecSocket(AF_UNIX, SOCK_STREAM, 0) == -1);
  assert(errno == ENOSYS);

  int missing_pair[2] = {42, 43};
  errno = 0;
  assert(inheritance::CreateCloseOnExecSocketPair(AF_UNIX, SOCK_STREAM, 0,
                                                   missing_pair) == -1);
  assert(errno == ENOSYS);
  assert(missing_pair[0] == -1 && missing_pair[1] == -1);

  errno = 0;
  assert(inheritance::CreateCloseOnExecPipe(nullptr) == -1);
  assert(errno == EINVAL);

  errno = 0;
  assert(darwin_art_bionic_install_fd_inheritance_boundary(nullptr) == -1);
  assert(errno == EINVAL);
  assert(darwin_art_bionic_install_fd_inheritance_boundary(ForwardForTest) ==
         0);
  assert(darwin_art_bionic_install_fd_inheritance_boundary(ForwardForTest) ==
         0);
  errno = 0;
  assert(inheritance::RunFdOperation(nullptr, nullptr) == -1);
  assert(errno == EINVAL);
  errno = 0;
  assert(inheritance::RunFdOperation(&ResultAndErrnoForTest, nullptr) == 123);
  assert(errno == ERANGE);
  errno = 0;
  assert(darwin_art_bionic_install_fd_inheritance_boundary(
             DifferentBoundaryForTest) == -1);
  assert(errno == EBUSY);

  const int single =
      inheritance::CreateCloseOnExecSocket(AF_UNIX, SOCK_STREAM, 0);
  assert(single >= 0);
  AssertCloseOnExec(single);
  assert(boundary_calls == 2);
  assert(observed_operation != nullptr && observed_context != nullptr);
  assert(close(single) == 0);

  int socket_pair[2] = {-1, -1};
  assert(inheritance::CreateCloseOnExecSocketPair(AF_UNIX, SOCK_STREAM, 0,
                                                   socket_pair) == 0);
  AssertCloseOnExec(socket_pair[0]);
  AssertCloseOnExec(socket_pair[1]);
  const char socket_byte = 's';
  char socket_received = 0;
  assert(write(socket_pair[0], &socket_byte, sizeof(socket_byte)) == 1);
  assert(read(socket_pair[1], &socket_received, sizeof(socket_received)) == 1);
  assert(socket_received == socket_byte);
  ClosePair(socket_pair);

  int pipe_pair[2] = {-1, -1};
  assert(inheritance::CreateCloseOnExecPipe(pipe_pair) == 0);
  AssertCloseOnExec(pipe_pair[0]);
  AssertCloseOnExec(pipe_pair[1]);
  const char pipe_byte = 'p';
  char pipe_received = 0;
  assert(write(pipe_pair[1], &pipe_byte, sizeof(pipe_byte)) == 1);
  assert(read(pipe_pair[0], &pipe_received, sizeof(pipe_received)) == 1);
  assert(pipe_received == pipe_byte);
  ClosePair(pipe_pair);

  assert(boundary_calls == 4);
  assert(observed_operation != nullptr && observed_context != nullptr);
  return 0;
}
