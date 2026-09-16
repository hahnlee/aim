#include "peer_credentials.h"
#include <cerrno>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

namespace darwin_art::binder {
bool ReadPeerCredentials(int socket, PeerCredentials* out) {
  if (out == nullptr) { errno = EINVAL; return false; }
  PeerCredentials peer{};
  socklen_t length = sizeof(peer.pid);
  if (getsockopt(socket, SOL_LOCAL, LOCAL_PEERPID, &peer.pid, &length) != 0) return false;
  if (length != sizeof(peer.pid) || peer.pid <= 0) { errno = EPROTO; return false; }
  if (getpeereid(socket, &peer.host_uid, &peer.host_gid) != 0) return false;
  *out = peer;
  return true;
}
}
