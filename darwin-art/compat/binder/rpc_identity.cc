#include "rpc_identity.h"

#include "calling_identity.h"
#include "peer_credentials.h"
#include "process_registry.h"

#include <cerrno>
#include <memory>
#include <mutex>
#include <new>
#include <unordered_map>
#include <vector>

namespace {

struct SessionIdentity {
  int32_t pid;
  int32_t android_uid;
  uid_t host_uid;
};

std::mutex g_sessions_mutex;
std::unordered_map<const void*, SessionIdentity> g_sessions;
thread_local std::vector<
    std::unique_ptr<darwin_art::binder::IncomingIdentity>>
    g_identity_stack;
thread_local int32_t g_strict_mode_policy = 0;
thread_local int32_t g_work_source_uid = -1;
thread_local bool g_propagate_work_source = false;

int64_t SetWorkSource(int32_t uid) {
  constexpr int64_t kPropagatedBit = int64_t{1} << 32;
  const int64_t token =
      (g_propagate_work_source ? kPropagatedBit : 0) |
      static_cast<uint32_t>(g_work_source_uid);
  g_work_source_uid = uid;
  g_propagate_work_source = true;
  return token;
}

}  // namespace

extern "C" bool darwin_art_binder_rpc_identity_bind(const void* session,
                                                     int socket_fd) {
  if (session == nullptr || socket_fd < 0) {
    errno = EINVAL;
    return false;
  }
  darwin_art::binder::PeerCredentials peer{};
  if (!darwin_art::binder::ReadPeerCredentials(socket_fd, &peer)) return false;
  const int32_t android_uid =
      darwin_art_runtime_registered_process_uid(static_cast<uint32_t>(peer.pid));
  if (android_uid < 0) {
    errno = EACCES;
    return false;
  }
  const SessionIdentity identity{static_cast<int32_t>(peer.pid), android_uid,
                                 peer.host_uid};
  std::lock_guard<std::mutex> lock(g_sessions_mutex);
  const auto [entry, inserted] = g_sessions.emplace(session, identity);
  if (inserted) return true;
  if (entry->second.pid == identity.pid &&
      entry->second.android_uid == identity.android_uid &&
      entry->second.host_uid == identity.host_uid) {
    return true;
  }
  errno = EPERM;
  return false;
}

extern "C" void darwin_art_binder_rpc_identity_forget(const void* session) {
  std::lock_guard<std::mutex> lock(g_sessions_mutex);
  g_sessions.erase(session);
}

extern "C" bool darwin_art_binder_rpc_identity_enter(const void* session,
                                                      uint32_t flags) {
  SessionIdentity identity{};
  {
    std::lock_guard<std::mutex> lock(g_sessions_mutex);
    const auto entry = g_sessions.find(session);
    if (entry == g_sessions.end()) return false;
    identity = entry->second;
  }
  try {
    // Linux Binder reports sender_pid=0 for asynchronous transactions while
    // retaining the authenticated Android UID.
    const int32_t pid = (flags & 1U) == 0 ? identity.pid : 0;
    g_identity_stack.push_back(
        std::make_unique<darwin_art::binder::IncomingIdentity>(
            pid, identity.android_uid));
  } catch (const std::bad_alloc&) {
    return false;
  }
  return true;
}

extern "C" void darwin_art_binder_rpc_identity_leave() {
  if (!g_identity_stack.empty()) g_identity_stack.pop_back();
}

extern "C" bool darwin_art_binder_rpc_identity_current(
    int32_t* pid, int32_t* uid, bool* explicit_identity) {
  const auto identity = darwin_art::binder::CurrentIdentity();
  if (!identity.remote) return false;
  if (pid != nullptr) *pid = identity.pid;
  if (uid != nullptr) *uid = identity.uid;
  if (explicit_identity != nullptr) {
    *explicit_identity = identity.explicit_identity;
  }
  return true;
}

extern "C" bool darwin_art_binder_rpc_identity_effective(
    int32_t* pid, int32_t* uid, bool* explicit_identity) {
  const auto identity = darwin_art::binder::CurrentIdentity();
  if (identity.uid < 0) return false;
  if (pid != nullptr) *pid = identity.pid;
  if (uid != nullptr) *uid = identity.uid;
  if (explicit_identity != nullptr) {
    *explicit_identity = identity.explicit_identity;
  }
  return true;
}

extern "C" bool darwin_art_binder_rpc_identity_clear(int64_t* token) {
  if (token == nullptr || darwin_art::binder::CurrentIdentity().uid < 0)
    return false;
  *token = darwin_art::binder::ClearIdentity();
  return true;
}

extern "C" bool darwin_art_binder_rpc_identity_restore(int64_t token) {
  if (darwin_art::binder::CurrentIdentity().uid < 0) return false;
  darwin_art::binder::RestoreIdentity(token);
  return true;
}

extern "C" bool darwin_art_binder_rpc_strict_mode_get(int32_t* policy) {
  if (policy == nullptr) return false;
  *policy = g_strict_mode_policy;
  return true;
}

extern "C" bool darwin_art_binder_rpc_strict_mode_set(int32_t policy) {
  g_strict_mode_policy = policy;
  return true;
}

extern "C" bool darwin_art_binder_rpc_work_source_set(int32_t uid,
                                                       int64_t* token) {
  if (token == nullptr) return false;
  *token = SetWorkSource(uid);
  return true;
}

extern "C" bool darwin_art_binder_rpc_work_source_get(int32_t* uid) {
  if (uid == nullptr) return false;
  *uid = g_work_source_uid;
  return true;
}

extern "C" bool darwin_art_binder_rpc_work_source_clear(int64_t* token) {
  return darwin_art_binder_rpc_work_source_set(-1, token);
}

extern "C" bool darwin_art_binder_rpc_work_source_restore(int64_t token) {
  g_work_source_uid = static_cast<int32_t>(static_cast<uint32_t>(token));
  g_propagate_work_source = (static_cast<uint64_t>(token) >> 32U) != 0;
  return true;
}
