#ifndef DARWIN_ART_BIONIC_FD_INHERITANCE_H_
#define DARWIN_ART_BIONIC_FD_INHERITANCE_H_

#include <stdint.h>

namespace darwin_art::bionic::fd_inheritance {

// The operation is synchronous and borrows context only for the duration of
// the call.  It must not wait for input, spawn, or re-enter this boundary.
using FdOperation = intptr_t (*)(void *context);

// This callback is installed by the trusted host/runtime owner.  It must
// execute |operation| while holding the process-wide Rust inheritance guard,
// then return the operation's result and native errno unchanged.
using FdInheritanceBoundary = intptr_t (*)(FdOperation operation,
                                           void *context);

// Delegate one synchronous native FD operation to the installed Rust-backed
// boundary.  A missing boundary fails closed with ENOSYS; a null operation
// fails with EINVAL.  This function does not provide a local fallback lock.
intptr_t RunFdOperation(FdOperation operation, void *context) noexcept;

// Create a native socket, then make it close-on-exec while still inside the
// installed inheritance boundary.  Returns the native descriptor or -1.
int CreateCloseOnExecSocket(int domain, int type, int protocol) noexcept;

// Create a socketpair/pipe and make both descriptors close-on-exec while still
// inside the installed inheritance boundary.  On failure, |descriptors| is
// left as {-1, -1} and every descriptor created by the operation is closed.
int CreateCloseOnExecSocketPair(int domain, int type, int protocol,
                                int descriptors[2]) noexcept;
int CreateCloseOnExecPipe(int descriptors[2]) noexcept;

} // namespace darwin_art::bionic::fd_inheritance

// Trusted host installer ABI.  Installation is immutable: repeating the same
// function pointer succeeds, while a different pointer fails with EBUSY.
// Passing null fails with EINVAL.
extern "C" int darwin_art_bionic_install_fd_inheritance_boundary(
    darwin_art::bionic::fd_inheritance::FdInheritanceBoundary boundary);

#endif // DARWIN_ART_BIONIC_FD_INHERITANCE_H_
