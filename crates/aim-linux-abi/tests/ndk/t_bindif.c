// A socket bound to eth0 (SO_BINDTODEVICE) stays bound to the device when
// the Mac's interface behind it changes: the boot's test hook (argv[1])
// puts the host's lo0 behind eth0, and the bound socket's sends to an
// outside address then find no route, as lo0 has none; back on the Mac's
// interface they go out again.
#include <arpa/inet.h>
#include <fcntl.h>
#include <net/if.h>
#include <netinet/in.h>
#include <sys/socket.h>
#include <time.h>
#include <unistd.h>

#include "check.h"

static const char* hook_path;

static void hook(const char* text) {
  int f = open(hook_path, O_WRONLY | O_TRUNC | O_CREAT | O_CLOEXEC, 0644);
  CHECK(f >= 0 && write(f, text, strlen(text)) == (ssize_t)strlen(text));
  close(f);
}

// Whether a datagram to TEST-NET-1 leaves the socket.
static int sends(int s) {
  struct sockaddr_in to = {.sin_family = AF_INET, .sin_port = htons(9)};
  inet_pton(AF_INET, "192.0.2.1", &to.sin_addr);
  return sendto(s, "x", 1, 0, (struct sockaddr*)&to, sizeof to) == 1;
}

// Whether `sends` turns `want` within 10 s (the network is watched every
// 2 s).
static int becomes(int s, int want) {
  for (int i = 0; i < 100; i++) {
    if (sends(s) == want) return 1;
    struct timespec t = {0, 100 * 1000 * 1000};
    nanosleep(&t, NULL);
  }
  return 0;
}

static void bound_socket_follows_the_device(void) {
  int s = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0);
  CHECK(s >= 0);
  CHECK(setsockopt(s, SOL_SOCKET, SO_BINDTODEVICE, "eth0", 5) == 0);
  CHECK(sends(s));
  hook("interface lo0\n");
  CHECK(becomes(s, 0));
  char name[IFNAMSIZ] = {0};
  socklen_t len = sizeof name;
  CHECK(getsockopt(s, SOL_SOCKET, SO_BINDTODEVICE, name, &len) == 0 && !strcmp(name, "eth0"));
  hook("");
  CHECK(becomes(s, 1));
  close(s);
}

int main(int argc, char** argv) {
  if (argc != 2) return 2;
  hook_path = argv[1];
  RUN(bound_socket_follows_the_device);
  DONE();
}
