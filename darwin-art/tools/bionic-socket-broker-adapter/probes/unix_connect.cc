#include "darwin_art_bionic_socket_broker.h"
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>
#include <poll.h>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cstddef>

extern "C" int darwin_art_unix_connect_smoke(void) {
  char directory[] = "/tmp/darwin-unix-connect.XXXXXX";
  if (!mkdtemp(directory)) return 1;
  sockaddr_un host{};
  host.sun_family = AF_UNIX;
  std::snprintf(host.sun_path, sizeof(host.sun_path), "%s/socket", directory);
  host.sun_len = offsetof(sockaddr_un, sun_path) + std::strlen(host.sun_path) + 1;
  const int server = socket(AF_UNIX, SOCK_STREAM, 0);
  if (server < 0) return 2;
  int result = 3;
  int guest = -1, peer = -1;
  do {
    if (bind(server, reinterpret_cast<sockaddr*>(&host), host.sun_len) != 0 ||
        listen(server, 1) != 0) break;
    struct { uint16_t family; char path[108]; } android{1, "/dev/socket/test-service"};
    const uint32_t length = 2 + std::strlen(android.path) + 1;
    guest = darwin_art_bionic_socket_broker_socket(1, 1, 0);
    if (guest < 0) break;
    // Existence of a host socket never authorizes an unregistered guest name.
    if (darwin_art_bionic_socket_broker_connect(guest, &android, length) != -1) break;
    if (darwin_art_bionic_socket_broker_install_unix_endpoint(
            &android, length, host.sun_path) != 0 ||
        darwin_art_bionic_socket_broker_connect(guest, &android, length) != 0) break;
    pollfd ready{server, POLLIN, 0};
    if (poll(&ready, 1, 1000) != 1) break;
    peer = accept(server, nullptr, nullptr);
    if (peer < 0) break;
    if (darwin_art_bionic_socket_broker_send(guest, "x", 1, 0) != 1) break;
    ready = {peer, POLLIN, 0};
    if (poll(&ready, 1, 1000) != 1) break;
    char byte = 0;
    if (read(peer, &byte, 1) != 1 || byte != 'x') break;
    result = 0;
  } while (false);
  if (guest >= 0) darwin_art_bionic_socket_broker_close(guest);
  if (peer >= 0) close(peer);
  close(server);
  unlink(host.sun_path);
  rmdir(directory);
  if (result == 0) std::fprintf(stderr, "guest Unix endpoint connect/send PASS\n");
  return result;
}
