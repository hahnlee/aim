// Pseudo-terminals (pty(7), posix_openpt(3), tty_ioctl(4)): what adbd's
// subprocesses do with them (a master, a slave in a new session as its
// controlling terminal, raw mode, the window size).
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdlib.h>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <sys/wait.h>
#include <termios.h>
#include <time.h>
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
  errno = 0;
  CHECK(open("/dev/ptmx", O_RDWR | O_DIRECTORY) == -1 && errno == ENOTDIR);
  errno = 0;
  CHECK(open("/dev/ptmx", O_RDWR | O_CREAT | O_EXCL, 0600) == -1 && errno == EEXIST);
  errno = 0;
  CHECK(open(name, O_RDWR | O_DIRECTORY) == -1 && errno == ENOTDIR);
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

// An owned child exits before the master consumes its small completed write.
static void unread_output_exit(void) {
  char name[64];
  int master = open_master(name, sizeof(name));
  CHECK(master >= 0);
  int progress[2];
  CHECK(pipe(progress) == 0);
  pid_t pid = fork();
  CHECK(pid >= 0);
  if (pid == 0) {
    close(progress[0]);
    close(master);
    alarm(5);
    if (setsid() < 0) _exit(1);
    int slave = open(name, O_RDWR);
    if (slave < 0) _exit(2);
    char output[268];
    memset(output, 'x', sizeof(output));
    if (write(slave, output, sizeof(output)) != (ssize_t)sizeof(output)) _exit(3);
    if (write(progress[1], "ready", 5) != 5) _exit(4);
    _exit(0);
  }
  close(progress[1]);
  char ready[5];
  CHECK(read(progress[0], ready, sizeof(ready)) == 5 && memcmp(ready, "ready", 5) == 0);
  close(progress[0]);
  int status = 0;
  pid_t result = 0;
  for (int n = 0; n < 100 && result == 0; n++) {
    result = waitpid(pid, &status, WNOHANG);
    if (result == 0) usleep(30000);
  }
  if (result == 0) {
    kill(pid, SIGKILL);
    waitpid(pid, &status, 0);
  }
  fprintf(stderr, "unread-output exit result=%d pid=%d status=%#x\n", result, pid, status);
  CHECK(result == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0);
  char retained[268];
  size_t received = 0;
  while (received < sizeof(retained)) {
    ssize_t count = read(master, retained + received, sizeof(retained) - received);
    CHECK(count > 0);
    received += count;
  }
  for (size_t n = 0; n < sizeof(retained); n++) CHECK(retained[n] == 'x');
  errno = 0;
  CHECK(read(master, retained, 1) == -1 && errno == EIO);
  close(master);
}

static volatile sig_atomic_t job_signal, job_code;
static void job_handler(int signal, siginfo_t* info, void* context) {
  (void)context;
  job_signal = signal;
  job_code = info->si_code;
}

static int job_control_session(void) {
  if (setsid() < 0) return 1;
  char name[64];
  int master = open_master(name, sizeof(name));
  if (master < 0) return 2;
  int slave = open(name, O_RDWR);
  if (slave < 0) return 3;
  struct termios mode;
  if (tcgetattr(slave, &mode) != 0) return 4;
  cfmakeraw(&mode);
  mode.c_lflag |= TOSTOP;
  if (tcsetattr(slave, TCSANOW, &mode) != 0) return 5;
  struct sigaction action = {.sa_sigaction = job_handler, .sa_flags = SA_SIGINFO};
  sigemptyset(&action.sa_mask);
  if (sigaction(SIGTTIN, &action, NULL) != 0 || sigaction(SIGTTOU, &action, NULL) != 0) return 6;
  pid_t background = fork();
  if (background < 0) return 7;
  if (background == 0) {
    alarm(8);
    if (setpgid(0, 0) != 0) _exit(10);
    char byte;
    errno = 0;
    if (read(slave, &byte, 1) != -1 || errno != EINTR || job_signal != SIGTTIN || job_code != SI_KERNEL) _exit(11);
    sigset_t blocked;
    sigemptyset(&blocked); sigaddset(&blocked, SIGTTIN);
    if (sigprocmask(SIG_BLOCK, &blocked, NULL) != 0) _exit(12);
    job_signal = 0; errno = 0;
    if (read(slave, &byte, 1) != -1 || errno != EIO || job_signal != 0) _exit(13);
    if (sigprocmask(SIG_UNBLOCK, &blocked, NULL) != 0 || signal(SIGTTIN, SIG_IGN) == SIG_ERR) _exit(14);
    errno = 0;
    if (read(slave, &byte, 1) != -1 || errno != EIO) _exit(15);
    job_signal = 0; errno = 0;
    if (write(slave, "w", 1) != -1 || errno != EINTR || job_signal != SIGTTOU || job_code != SI_KERNEL) _exit(16);
    sigemptyset(&blocked); sigaddset(&blocked, SIGTTOU);
    if (sigprocmask(SIG_BLOCK, &blocked, NULL) != 0 || write(slave, "b", 1) != 1) _exit(17);
    if (sigprocmask(SIG_UNBLOCK, &blocked, NULL) != 0 || signal(SIGTTOU, SIG_IGN) == SIG_ERR || write(slave, "i", 1) != 1) _exit(18);
    mode.c_lflag &= ~TOSTOP;
    if (tcsetattr(slave, TCSANOW, &mode) != 0 || write(slave, "n", 1) != 1) _exit(19);
    _exit(0);
  }
  int status;
  if (waitpid(background, &status, 0) != background || !WIFEXITED(status) || WEXITSTATUS(status) != 0) {
    fprintf(stderr, "background job status=%#x\n", status); return 20;
  }
  mode.c_lflag |= TOSTOP;
  if (tcsetattr(slave, TCSANOW, &mode) != 0) return 21;
  int ready[2], release[2];
  if (pipe(ready) != 0 || pipe(release) != 0) return 22;
  pid_t foreground = fork();
  if (foreground < 0) return 23;
  if (foreground == 0) {
    alarm(8); close(ready[0]); close(release[1]);
    char byte;
    if (setpgid(0, 0) != 0 || write(ready[1], "r", 1) != 1 || read(release[0], &byte, 1) != 1) _exit(24);
    _exit(0);
  }
  close(ready[1]); close(release[0]);
  char byte;
  if (read(ready[0], &byte, 1) != 1 || tcsetpgrp(slave, foreground) != 0) return 25;
  job_signal = 0; errno = 0;
  if (read(slave, &byte, 1) != -1 || errno != EIO || job_signal != 0) return 26;
  errno = 0;
  if (write(slave, "o", 1) != -1 || errno != EIO || job_signal != 0) return 27;
  sigset_t blocked;
  sigemptyset(&blocked); sigaddset(&blocked, SIGTTOU);
  if (sigprocmask(SIG_BLOCK, &blocked, NULL) != 0 || tcsetpgrp(slave, getpid()) != 0) return 28;
  if (write(release[1], "r", 1) != 1 || waitpid(foreground, &status, 0) != foreground || !WIFEXITED(status) || WEXITSTATUS(status) != 0) return 29;
  close(ready[0]); close(release[1]); close(slave); close(master);
  return 0;
}
static void background_job_control(void) {
  pid_t child = fork();
  CHECK(child >= 0);
  if (child == 0) { alarm(15); _exit(job_control_session()); }
  int status;
  CHECK(waitpid(child, &status, 0) == child);
  fprintf(stderr, "TTY job-control status=%#x\n", status);
  CHECK(WIFEXITED(status) && WEXITSTATUS(status) == 0);
}

static void raw_read_timer(void) {
  char name[64];
  int master = open_master(name, sizeof(name));
  CHECK(master >= 0);
  int slave = open(name, O_RDWR | O_NOCTTY);
  CHECK(slave >= 0);
  struct termios mode;
  CHECK(tcgetattr(slave, &mode) == 0);
  cfmakeraw(&mode);
  mode.c_cc[VMIN] = 0;
  mode.c_cc[VTIME] = 1;
  CHECK(tcsetattr(slave, TCSANOW, &mode) == 0);
  struct timespec before, after;
  CHECK(clock_gettime(CLOCK_MONOTONIC, &before) == 0);
  alarm(3);
  char byte;
  CHECK(read(slave, &byte, 1) == 0);
  alarm(0);
  CHECK(clock_gettime(CLOCK_MONOTONIC, &after) == 0);
  double elapsed = (after.tv_sec - before.tv_sec) + (after.tv_nsec - before.tv_nsec) / 1e9;
  CHECK(elapsed >= .07 && elapsed < 2);
  mode.c_cc[VTIME] = 0;
  CHECK(tcsetattr(slave, TCSANOW, &mode) == 0);
  CHECK(read(slave, &byte, 1) == 0);
  mode.c_cc[VMIN] = 1;
  CHECK(tcsetattr(slave, TCSANOW, &mode) == 0);
  struct sigaction action = {.sa_sigaction = job_handler, .sa_flags = SA_SIGINFO};
  sigemptyset(&action.sa_mask);
  CHECK(sigaction(SIGALRM, &action, NULL) == 0);
  struct itimerval timer = {.it_value = {.tv_usec = 100000}};
  job_signal = 0;
  CHECK(setitimer(ITIMER_REAL, &timer, NULL) == 0);
  errno = 0;
  CHECK(read(slave, &byte, 1) == -1 && errno == EINTR && job_signal == SIGALRM);
  memset(&timer, 0, sizeof(timer));
  CHECK(setitimer(ITIMER_REAL, &timer, NULL) == 0);
  CHECK(close(slave) == 0 && close(master) == 0);
}

static const char* executable;
static void opaque_exec_description(void) {
  char name[64];
  int master = open_master(name, sizeof(name));
  CHECK(master >= 0);
  int slave = open(name, O_RDWR | O_NOCTTY | O_NONBLOCK);
  CHECK(slave >= 0);
  pid_t pid = fork();
  CHECK(pid >= 0);
  if (pid == 0) {
    close(master);
    alarm(10);
    if (setuid(2000) != 0) _exit(5);
    char fd[32];
    snprintf(fd, sizeof(fd), "%d", slave);
    execl(executable, executable, "--write-fd", fd, "2000", NULL);
    _exit(1);
  }
  CHECK(close(slave) == 0);
  struct pollfd ready = {.fd = master, .events = POLLIN};
  CHECK(poll(&ready, 1, 5000) == 1);
  char output[270];
  size_t received = 0;
  while (received < sizeof(output)) {
    ssize_t count = read(master, output + received, sizeof(output) - received);
    CHECK(count > 0);
    received += count;
  }
  CHECK(output[0] == 'e' && output[1] == 'x');
  for (size_t n = 2; n < sizeof(output); n++) CHECK(output[n] == 'x');
  int status;
  CHECK(waitpid(pid, &status, 0) == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0);
  errno = 0;
  CHECK(read(master, output, 1) == -1 && errno == EIO);
  CHECK(close(master) == 0);
}

static void allocate_and_close(void) {
  char name[64];
  int master = open_master(name, sizeof(name));
  CHECK(master >= 0);
  CHECK(close(master) == 0);
}

int main(int argc, char** argv) {
  executable = argv[0];
  if (argc == 4 && strcmp(argv[1], "--write-fd") == 0) {
    if (getuid() != (uid_t)atoi(argv[3]) || geteuid() != (uid_t)atoi(argv[3])) return 6;
    errno = 0;
    if (setuid(0) != -1 || errno != EPERM) return 7;
    int fd = atoi(argv[2]);
    if (fcntl(fd, F_GETFD) != 0 || !(fcntl(fd, F_GETFL) & O_NONBLOCK)) return 2;
    char output[268];
    memset(output, 'x', sizeof(output));
    if (write(fd, "ex", 2) != 2 || write(fd, output, sizeof(output)) != sizeof(output)) return 3;
    if (close(fd) != 0) return 4;
    return 0;
  }
  if (argc == 2 && strcmp(argv[1], "--allocate-close") == 0) {
    RUN(allocate_and_close);
    DONE();
  }
  RUN(master_and_slave);
  RUN(not_a_terminal);
  RUN(controlling_terminal);
  RUN(unread_output_exit);
  RUN(opaque_exec_description);
  RUN(raw_read_timer);
  RUN(background_job_control);
  DONE();
}
