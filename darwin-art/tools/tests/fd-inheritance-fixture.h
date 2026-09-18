#pragma once
#include "../bionic-socket-broker-adapter/src/fd_inheritance.h"

// Isolated native fixtures below never fork/spawn. They explicitly install a
// forwarding port to exercise production FD factories; this is NOT evidence
// for the production Rust lock/spawn closure and is never a runtime input.
namespace darwin_art::test {
inline intptr_t
ExecuteNoSpawnFdFixture(bionic::fd_inheritance::FdOperation operation,
                        void *context) {
  return operation(context);
}
inline int InstallNoSpawnFdFixture() {
  return darwin_art_bionic_install_fd_inheritance_boundary(
      &ExecuteNoSpawnFdFixture);
}
} // namespace darwin_art::test
