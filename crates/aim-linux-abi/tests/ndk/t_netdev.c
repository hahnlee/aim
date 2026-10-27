// proc_pid_net(5): the device inventory, exact headers and 16 counters,
// including fresh host loopback totals after UDP traffic (#268).
#include <arpa/inet.h>
#include <dirent.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdlib.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <unistd.h>

#include "check.h"

static const char headers[] =
    "Inter-|   Receive                                                |  Transmit\n"
    " face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop "
    "fifo colls carrier compressed\n";
static uint64_t loopback[16];

static void table(const char* path) {
  struct stat st;
  CHECK(stat(path, &st) == 0 && S_ISREG(st.st_mode) && (st.st_mode & 0777) == 0444);
  int fd = open(path, O_RDONLY | O_CLOEXEC);
  CHECK(fd >= 0);
  char buf[4096];
  int used = 0;
  ssize_t n;
  // Small reads also exercise file offsets and EOF.
  while ((n = read(fd, buf + used, 7)) > 0) {
    used += n;
    CHECK(used + 7 < (int)sizeof buf);
  }
  CHECK(n == 0);
  buf[used] = 0;
  CHECK(lseek(fd, 0, SEEK_SET) == 0);
  char first[sizeof headers - 1];
  CHECK(read(fd, first, sizeof first) == sizeof first && memcmp(first, headers, sizeof first) == 0);
  CHECK(close(fd) == 0);
  CHECK(used > (int)strlen(headers) && memcmp(buf, headers, strlen(headers)) == 0);
  int lo = 0, eth = 0;
  char* rest = buf + strlen(headers);
  for (char* row = strtok_r(rest, "\n", &rest); row; row = strtok_r(NULL, "\n", &rest)) {
    char* colon = strchr(row, ':');
    CHECK(colon);
    *colon = 0;
    while (*row == ' ') row++;
    CHECK(strcmp(row, "lo") == 0 || strcmp(row, "eth0") == 0);
    uint64_t values[16];
    char* p = colon + 1;
    for (int i = 0; i < 16; i++) {
      while (*p == ' ') p++;
      CHECK(*p >= '0' && *p <= '9');
      char* end;
      errno = 0;
      values[i] = strtoull(p, &end, 10);
      CHECK(end != p && errno == 0);
      p = end;
    }
    CHECK(*p == 0);
    // FIFO, frame, compression and carrier errors have no host counters.
    CHECK(values[4] == 0 && values[5] == 0 && values[6] == 0 && values[12] == 0 &&
          values[14] == 0 && values[15] == 0);
    if (strcmp(row, "lo") == 0) {
      lo++;
      memcpy(loopback, values, sizeof values);
    } else {
      eth++;
    }
  }
  CHECK(lo == 1 && eth == 1);
}

static void inventory(void) {
  DIR* d = opendir("/proc/net");
  CHECK(d);
  struct dirent* e;
  int found = 0;
  while ((e = readdir(d))) found += strcmp(e->d_name, "dev") == 0;
  CHECK(closedir(d) == 0 && found == 1);
  table("/proc/net/dev");
  table("/proc/self/net/dev");
  char path[128];
  snprintf(path, sizeof path, "/proc/%d/net/dev", getpid());
  table(path);
  snprintf(path, sizeof path, "/proc/self/task/%d/net/dev", gettid());
  table(path);
}

static void live_counters(void) {
  table("/proc/net/dev");
  uint64_t before[16];
  memcpy(before, loopback, sizeof before);
  int r = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0);
  int s = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0);
  CHECK(r >= 0 && s >= 0);
  struct sockaddr_in a = {.sin_family = AF_INET, .sin_addr.s_addr = htonl(INADDR_LOOPBACK)};
  socklen_t len = sizeof a;
  CHECK(bind(r, (struct sockaddr*)&a, len) == 0 && getsockname(r, (struct sockaddr*)&a, &len) == 0);
  // Large enough to change Darwin's byte totals even when rounded to KiB.
  char data[4096] = {0};
  CHECK(sendto(s, data, sizeof data, 0, (struct sockaddr*)&a, len) == sizeof data);
  CHECK(recv(r, data, sizeof data, 0) == sizeof data);
  CHECK(close(r) == 0 && close(s) == 0);
  table("/proc/net/dev");
  CHECK(loopback[0] >= before[0] + sizeof data && loopback[8] >= before[8] + sizeof data);
  CHECK(loopback[1] > before[1] && loopback[9] > before[9]);
}

int main(void) {
  RUN(inventory);
  RUN(live_counters);
  DONE();
}
