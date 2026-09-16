#include "../compat/binder/peer_credentials.h"
#include <cassert>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <sys/socket.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <unistd.h>

int main() {
  char directory[] = "/tmp/darwin-peer.XXXXXX";
  assert(mkdtemp(directory));
  sockaddr_un address{};
  address.sun_family = AF_UNIX;
  snprintf(address.sun_path, sizeof(address.sun_path), "%s/socket", directory);
  int listener = socket(AF_UNIX, SOCK_STREAM, 0);
  assert(listener >= 0);
  assert(bind(listener, reinterpret_cast<sockaddr*>(&address), sizeof(address)) == 0);
  assert(listen(listener, 1) == 0);
  int inherited[2];
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, inherited) == 0);
  pid_t child = fork();
  assert(child >= 0);
  if (child == 0) {
    close(inherited[0]);
    assert(write(inherited[1], "c", 1) == 1);
    close(inherited[1]);
    close(listener);
    int client = socket(AF_UNIX, SOCK_STREAM, 0);
    assert(client >= 0);
    assert(connect(client, reinterpret_cast<sockaddr*>(&address), sizeof(address)) == 0);
    char done;
    assert(read(client, &done, 1) == 1);
    close(client);
    _exit(0);
  }
  close(inherited[1]);
  char from_child;
  assert(read(inherited[0], &from_child, 1) == 1);
  darwin_art::binder::PeerCredentials inherited_peer{};
  // Darwin does not supply authenticated peer credentials for this inherited
  // socketpair. Do not substitute the server PID when the query fails.
  assert(!darwin_art::binder::ReadPeerCredentials(inherited[0], &inherited_peer));
  close(inherited[0]);
  int accepted = accept(listener, nullptr, nullptr);
  assert(accepted >= 0);
  darwin_art::binder::PeerCredentials peer{};
  assert(darwin_art::binder::ReadPeerCredentials(accepted, &peer));
  assert(peer.pid == child && peer.pid != getpid());
  assert(peer.host_uid == geteuid() && peer.host_gid == getegid());
  // Invalid descriptors fail rather than inventing the server PID/system UID.
  assert(!darwin_art::binder::ReadPeerCredentials(-1, &peer));
  assert(write(accepted, "x", 1) == 1);
  close(accepted);
  close(listener);
  int status;
  assert(waitpid(child, &status, 0) == child);
  assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
  assert(unlink(address.sun_path) == 0);
  assert(rmdir(directory) == 0);
  puts("Binder accepted-connection peer PID/host credentials PASS; Android UID mapping pending");
}
