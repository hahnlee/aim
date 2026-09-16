#include "binder/calling_identity.h"
#include "binder/rpc_identity.h"

#include <atomic>
#include <cassert>
#include <cerrno>
#include <cstdint>
#include <cstdio>
#include <sys/socket.h>
#include <unistd.h>

namespace {
std::atomic<bool> g_registered{true};
constexpr int32_t kAndroidUid = 10123;
}

extern "C" int32_t darwin_art_runtime_registered_process_uid(uint32_t pid) {
  return g_registered.load() && pid == static_cast<uint32_t>(getpid())
             ? kAndroidUid
             : -1;
}

int main() {
  int sockets[2];
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sockets) == 0);
  const int session = 1;
  assert(darwin_art_binder_rpc_identity_bind(&session, sockets[0]));

  int32_t pid = -1;
  int32_t uid = -1;
  bool explicit_identity = true;
  assert(!darwin_art_binder_rpc_identity_current(&pid, &uid,
                                                  &explicit_identity));
  assert(darwin_art_binder_rpc_identity_effective(&pid, &uid,
                                                  &explicit_identity));
  assert(pid == getpid() && uid == kAndroidUid && !explicit_identity);
  int64_t local_token = 0;
  assert(darwin_art_binder_rpc_identity_clear(&local_token));
  assert(darwin_art_binder_rpc_identity_effective(&pid, &uid,
                                                  &explicit_identity));
  assert(pid == getpid() && uid == kAndroidUid && explicit_identity);
  assert(darwin_art_binder_rpc_identity_restore(local_token));
  assert(darwin_art_binder_rpc_identity_enter(&session, 0));
  assert(darwin_art_binder_rpc_identity_current(&pid, &uid,
                                                 &explicit_identity));
  assert(pid == getpid() && uid == kAndroidUid && !explicit_identity);

  int32_t strict_policy = -1;
  assert(darwin_art_binder_rpc_strict_mode_get(&strict_policy));
  assert(strict_policy == 0);
  assert(darwin_art_binder_rpc_strict_mode_set(0x1234));
  assert(darwin_art_binder_rpc_strict_mode_get(&strict_policy));
  assert(strict_policy == 0x1234);

  int64_t work_source_token = 0;
  int32_t work_source = 0;
  assert(darwin_art_binder_rpc_work_source_set(1042, &work_source_token));
  assert(darwin_art_binder_rpc_work_source_get(&work_source));
  assert(work_source == 1042);
  int64_t cleared_work_source = 0;
  assert(darwin_art_binder_rpc_work_source_clear(&cleared_work_source));
  assert(darwin_art_binder_rpc_work_source_get(&work_source));
  assert(work_source == -1);
  assert(darwin_art_binder_rpc_work_source_restore(cleared_work_source));
  assert(darwin_art_binder_rpc_work_source_get(&work_source));
  assert(work_source == 1042);
  assert(darwin_art_binder_rpc_work_source_restore(work_source_token));
  assert(darwin_art_binder_rpc_work_source_get(&work_source));
  assert(work_source == -1);

  int64_t token = 0;
  assert(darwin_art_binder_rpc_identity_clear(&token));
  assert(darwin_art_binder_rpc_identity_current(&pid, &uid,
                                                 &explicit_identity));
  assert(pid == getpid() && uid == kAndroidUid && explicit_identity);
  assert(darwin_art_binder_rpc_identity_restore(token));
  assert(darwin_art_binder_rpc_identity_current(&pid, &uid,
                                                 &explicit_identity));
  assert(pid == getpid() && uid == kAndroidUid && !explicit_identity);
  darwin_art_binder_rpc_identity_leave();
  assert(!darwin_art_binder_rpc_identity_current(nullptr, nullptr, nullptr));
  assert(darwin_art_binder_rpc_strict_mode_get(&strict_policy));
  assert(strict_policy == 0x1234);
  assert(darwin_art_binder_rpc_work_source_get(&work_source));
  assert(work_source == -1);

  assert(darwin_art_binder_rpc_identity_enter(&session, 1));
  assert(darwin_art_binder_rpc_identity_current(&pid, &uid, nullptr));
  assert(pid == 0 && uid == kAndroidUid);
  darwin_art_binder_rpc_identity_leave();
  darwin_art_binder_rpc_identity_forget(&session);
  assert(!darwin_art_binder_rpc_identity_enter(&session, 0));

  const int denied_session = 2;
  g_registered.store(false);
  errno = 0;
  assert(!darwin_art_binder_rpc_identity_bind(&denied_session, sockets[0]));
  assert(errno == EACCES);
  close(sockets[0]);
  close(sockets[1]);
  std::puts("Binder RPC identity: peer registry, sync/oneway, clear/restore PASS");
}
