#include "calling_identity.h"
#include "process_registry.h"
#include <unistd.h>

namespace darwin_art::binder {
namespace {
Identity Self() {
  static const int32_t uid = darwin_art_runtime_registered_process_uid(getpid());
  return {static_cast<int32_t>(getpid()), uid, false, false};
}
thread_local Identity identity = Self();
}
Identity CurrentIdentity() { return identity; }
IncomingIdentity::IncomingIdentity(int32_t pid, int32_t uid) : previous_(identity) {
  identity = {pid, uid, false, true};
}
IncomingIdentity::~IncomingIdentity() { identity = previous_; }

int64_t ClearIdentity() {
  // Android IPCThreadState token: UID high word, explicit-identity bit30 in
  // signed PID low word. Retain the transport scope separately from the token.
  const uint32_t pid = (static_cast<uint32_t>(identity.pid) & ~(1u << 30)) |
                       (identity.explicit_identity ? 1u << 30 : 0);
  const uint64_t token = (static_cast<uint64_t>(static_cast<uint32_t>(identity.uid)) << 32) | pid;
  const bool remote = identity.remote;
  identity = Self();
  identity.explicit_identity = true;
  identity.remote = remote;
  return static_cast<int64_t>(token);
}
void RestoreIdentity(int64_t token) {
  const uint64_t bits = static_cast<uint64_t>(token);
  const uint32_t low = static_cast<uint32_t>(bits);
  identity.uid = static_cast<int32_t>(bits >> 32);
  identity.pid = static_cast<int32_t>((low & (1u << 31)) ? low | (1u << 30) : low & ~(1u << 30));
  identity.explicit_identity = (low & (1u << 30)) != 0;
}
}
