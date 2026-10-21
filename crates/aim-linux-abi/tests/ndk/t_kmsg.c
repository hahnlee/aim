// The kernel log (/dev/kmsg, /proc/kmsg, syslog(2)): records written by one
// process read back by another, the record format, readers' positions,
// blocking and nonblocking reads, and syslog's actions and permissions.
#include <fcntl.h>
#include <poll.h>
#include <stdlib.h>
#include <sys/klog.h>
#include <sys/wait.h>
#include <unistd.h>

#include "check.h"

// Reads /dev/kmsg records from `fd` until one's text is `text`; its line.
static int find(int fd, const char* text, char* line, size_t size) {
  for (;;) {
    ssize_t n = read(fd, line, size - 1);
    if (n <= 0) return -1;
    line[n] = 0;
    char* body = strchr(line, ';');
    if (body && strncmp(body + 1, text, strlen(text)) == 0 && body[1 + strlen(text)] == '\n')
      return 0;
  }
}

static void records(void) {
  int r = open("/dev/kmsg", O_RDONLY | O_NONBLOCK);
  CHECK(r >= 0);
  CHECK(lseek(r, 0, SEEK_END) == 0);
  char line[2048];
  CHECK(read(r, line, sizeof line) < 0 && errno == EAGAIN);
  // O_TRUNC, as a shell's redirection opens it, leaves the log.
  int w = open("/dev/kmsg", O_WRONLY | O_TRUNC);
  CHECK(w >= 0);
  CHECK(write(w, "<3>first", 8) == 8);
  CHECK(write(w, "plain\n", 6) == 6);
  CHECK(write(w, "<14>two\nlines\\", 14) == 14);
  close(w);
  CHECK(read(r, line, sizeof line) > 0);
  unsigned prefix;
  unsigned long long seq, first, usec;
  char flag;
  int text;
  CHECK(sscanf(line, "%u,%llu,%llu,%c;%n", &prefix, &first, &usec, &flag, &text) == 4);
  CHECK(prefix == (1 << 3 | 3) && flag == '-' && strcmp(line + text, "first\n") == 0);
  CHECK(read(r, line, sizeof line) > 0);
  CHECK(sscanf(line, "%u,%llu,", &prefix, &seq) == 2 && prefix == 12 && seq == first + 1);
  CHECK(strstr(line, ";plain\n"));
  // A short buffer cannot take the record; it stays to be read.
  CHECK(read(r, line, 8) < 0 && errno == EINVAL);
  CHECK(read(r, line, sizeof line) > 0);
  CHECK(strstr(line, "14,") == line && strstr(line, ";two\\x0alines\\x5c\n"));
  CHECK(read(r, line, sizeof line) < 0 && errno == EAGAIN);
  // SEEK_SET goes back to the oldest record; other offsets are refused.
  CHECK(lseek(r, 0, SEEK_SET) == 0);
  CHECK(find(r, "first", line, sizeof line) == 0);
  CHECK(lseek(r, 5, SEEK_SET) < 0 && errno == ESPIPE);
  // Too long a record.
  w = open("/dev/kmsg", O_WRONLY);
  char big[1100];
  memset(big, 'x', sizeof big);
  CHECK(write(w, big, sizeof big) < 0 && errno == EINVAL);
  close(w);
  close(r);
}

static void blocking_reader(void) {
  int r = open("/dev/kmsg", O_RDONLY);
  CHECK(r >= 0);
  CHECK(lseek(r, 0, SEEK_END) == 0);
  FORK_OR_SKIP(child);
  if (child == 0) {
    usleep(100000);
    int w = open("/dev/kmsg", O_WRONLY);
    _exit(w >= 0 && write(w, "<6>from the child", 17) == 17 ? 0 : 1);
  }
  char line[256];
  ssize_t n = read(r, line, sizeof line - 1);
  CHECK(n > 0);
  line[n] = 0;
  CHECK(strstr(line, ";from the child\n"));
  int status;
  CHECK(waitpid(child, &status, 0) == child && WIFEXITED(status) && WEXITSTATUS(status) == 0);
  close(r);
}

static void syslog_actions(void) {
  CHECK(klogctl(10, NULL, 0) == 1 << 17);  // SIZE_BUFFER
  int w = open("/dev/kmsg", O_WRONLY);
  CHECK(w >= 0);
  CHECK(write(w, "<5>syslog marker", 16) == 16);
  close(w);
  char* buf = malloc(1 << 17);
  int n = klogctl(3, buf, 1 << 17);  // READ_ALL
  CHECK(n > 0);
  buf[n] = 0;
  CHECK(strstr(buf, "<13>[") && strstr(buf, "] syslog marker\n"));
  // The newest records that fit a small buffer.
  n = klogctl(3, buf, 40);
  CHECK(n > 0 && n <= 40);
  CHECK(klogctl(3, NULL, 10) < 0 && errno == EINVAL);
  CHECK(klogctl(3, buf, 0) == 0);
  CHECK(klogctl(8, NULL, 9) < 0 && errno == EINVAL);  // CONSOLE_LEVEL
  CHECK(klogctl(8, NULL, 4) == 0);
  CHECK(klogctl(99, NULL, 0) < 0 && errno == EINVAL);
  // Destructive reads: everything unread, then nothing.
  int unread = klogctl(9, NULL, 0);  // SIZE_UNREAD
  CHECK(unread > 0);
  int got = 0;
  while (got < unread) {
    n = klogctl(2, buf, 1 << 16);  // READ
    CHECK(n > 0);
    got += n;
  }
  CHECK(got == unread && klogctl(9, NULL, 0) == 0);
  // /proc/kmsg reads from the same position.
  int p = open("/proc/kmsg", O_RDONLY | O_NONBLOCK);
  CHECK(p >= 0);
  CHECK(read(p, buf, 4096) < 0 && errno == EAGAIN);
  w = open("/dev/kmsg", O_WRONLY);
  CHECK(write(w, "<4>to proc", 10) == 10);
  close(w);
  n = read(p, buf, 4096);
  CHECK(n > 0);
  buf[n] = 0;
  CHECK(strstr(buf, "<12>[") == buf && strstr(buf, "] to proc\n"));
  close(p);
  // CLEAR: READ_ALL starts after it.
  CHECK(klogctl(5, NULL, 0) == 0);
  CHECK(klogctl(3, buf, 4096) == 0);
  free(buf);
}

static void permissions(void) {
  FORK_OR_SKIP(child);
  if (child == 0) {
    if (setgid(2000) != 0 || setuid(2000) != 0) _exit(2);
    char buf[64];
    // Without CAP_SYSLOG only READ_ALL and SIZE_BUFFER (dmesg_restrict 0).
    int ok = klogctl(10, NULL, 0) > 0 && klogctl(3, buf, sizeof buf) >= 0 &&
             klogctl(2, buf, sizeof buf) < 0 && errno == EPERM &&
             klogctl(9, NULL, 0) < 0 && errno == EPERM &&
             open("/proc/kmsg", O_RDONLY) < 0 && errno == EPERM;
    _exit(ok ? 0 : 1);
  }
  int status;
  CHECK(waitpid(child, &status, 0) == child && WIFEXITED(status) && WEXITSTATUS(status) == 0);
}

int main(void) {
  RUN(records);
  RUN(blocking_reader);
  RUN(syslog_actions);
  RUN(permissions);
  DONE();
}
