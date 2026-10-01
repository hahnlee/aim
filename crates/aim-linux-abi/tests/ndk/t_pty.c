// Pseudo-terminals (pty(7), posix_openpt(3), tty_ioctl(4)): what adbd's
// subprocesses do with them (a master, a slave in a new session as its
// controlling terminal, raw mode, the window size).
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdlib.h>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <termios.h>
#include <unistd.h>

#include "check.h"

static int open_master(char* name, size_t size) {
  int master = posix_openpt(O_RDWR | O_NOCTTY | O_CLOEXEC);
  if (master < 0 || grantpt(master) != 0 || unlockpt(master) != 0 ||
      ptsname_r(master, name, size) != 0) {
    return -1;
  }
  return master;
}

static void master_and_slave(void) {
  char name[64];
  int master = open_master(name, sizeof(name));
  CHECK(master >= 0);
  CHECK(strncmp(name, "/dev/pts/", 9) == 0);
  unsigned int n = 0;
  CHECK(ioctl(master, TIOCGPTN, &n) == 0 && n == (unsigned int)atoi(name + 9));
  int slave = open(name, O_RDWR | O_NOCTTY | O_CLOEXEC);
  CHECK(slave >= 0);
  CHECK(isatty(slave) && isatty(master));
  struct stat st;
  CHECK(fstat(slave, &st) == 0 && S_ISCHR(st.st_mode));
  // The default line discipline: canonical, echoing, newline to CR-NL.
  struct termios t;
  CHECK(tcgetattr(slave, &t) == 0);
  CHECK((t.c_lflag & (ICANON | ECHO | ISIG)) == (ICANON | ECHO | ISIG));
  CHECK((t.c_oflag & (OPOST | ONLCR)) == (OPOST | ONLCR));
  CHECK((t.c_cflag & CSIZE) == CS8);
  CHECK(write(slave, "x\n", 2) == 2);
  char buf[16];
  CHECK(read(master, buf, sizeof(buf)) == 3 && memcmp(buf, "x\r\n", 3) == 0);
  // Raw, as adbd makes a pty for raw data.
  cfmakeraw(&t);
  CHECK(tcsetattr(slave, TCSADRAIN, &t) == 0);
  struct termios back;
  CHECK(tcgetattr(slave, &back) == 0);
  CHECK((back.c_lflag & (ICANON | ECHO | ISIG)) == 0 && (back.c_oflag & OPOST) == 0);
  CHECK(back.c_cc[VMIN] == 1 && back.c_cc[VTIME] == 0);
  CHECK(write(slave, "a\nb", 3) == 3);
  CHECK(read(master, buf, sizeof(buf)) == 3 && memcmp(buf, "a\nb", 3) == 0);
  CHECK(write(master, "in", 2) == 2);
  CHECK(read(slave, buf, sizeof(buf)) == 2 && memcmp(buf, "in", 2) == 0);
  // The window size, set on the master and read on the slave.
  struct winsize ws = {.ws_row = 24, .ws_col = 80};
  CHECK(ioctl(master, TIOCSWINSZ, &ws) == 0);
  struct winsize got;
  CHECK(ioctl(slave, TIOCGWINSZ, &got) == 0 && got.ws_row == 24 && got.ws_col == 80);
  close(slave);
  close(master);
}

static void not_a_terminal(void) {
  int p[2];
  CHECK(pipe(p) == 0);
  struct termios t;
  CHECK(tcgetattr(p[0], &t) == -1 && errno == ENOTTY);
  CHECK(!isatty(p[0]));
  unsigned int n;
  CHECK(ioctl(p[0], TIOCGPTN, &n) == -1 && errno == ENOTTY);
  close(p[0]);
  close(p[1]);
  CHECK(open("/dev/pts/999999", O_RDWR) == -1 && errno == ENOENT);
}

// adbd's child: in a new session, the slave opened without O_NOCTTY
// becomes its controlling terminal; what it writes before it exits reaches
// the master, read while it runs, as adbd reads.
static void controlling_terminal(void) {
  char name[64];
  int master = open_master(name, sizeof(name));
  CHECK(master >= 0);
  FORK_OR_SKIP(pid);
  if (pid == 0) {
    alarm(10);
    setsid();
    int slave = open(name, O_RDWR);
    if (slave < 0) _exit(1);
    if (tcgetsid(slave) != getpid()) _exit(2);
    if (tcgetpgrp(slave) != getpid()) _exit(3);
    if (write(slave, "ready", 5) != 5) _exit(4);
    _exit(0);
  }
  char buf[8];
  struct pollfd p = {.fd = master, .events = POLLIN};
  CHECK(poll(&p, 1, 5000) == 1 && read(master, buf, sizeof(buf)) == 5 &&
        memcmp(buf, "ready", 5) == 0);
  int status;
  CHECK(waitpid(pid, &status, 0) == pid);
  CHECK(WIFEXITED(status) && WEXITSTATUS(status) == 0);
  close(master);
}

int main(void) {
  RUN(master_and_slave);
  RUN(not_a_terminal);
  RUN(controlling_terminal);
  DONE();
}
