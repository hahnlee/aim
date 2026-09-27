// Process lifecycle checks run as a guest under linux-run (tests/process.rs).
// Built with the NDK for aarch64-linux-android; every check prints one
// "ok <name>" line or fails with a message and exit status 1.
//
// usage: process CHECK
//        process argv ARGS...   (prints its argv and AT_EXECFN; used as a
//                                script interpreter and exec target)

#include <errno.h>
#include <linux/filter.h>
#include <linux/seccomp.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/auxv.h>
#include <sys/capability.h>
#include <sys/epoll.h>
#include <sys/prctl.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/utsname.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#ifndef PIDFD_NONBLOCK
#define PIDFD_NONBLOCK O_NONBLOCK
#endif

#define CHECK(c, ...)                                   \
  do {                                                  \
    if (!(c)) {                                         \
      fprintf(stderr, "FAIL %s:%d: %s: ", __func__, __LINE__, #c); \
      fprintf(stderr, __VA_ARGS__);                     \
      fprintf(stderr, " (errno %d)\n", errno);          \
      exit(1);                                          \
    }                                                   \
  } while (0)

static const char* self_path;

static int pidfd_open(pid_t pid, unsigned flags) { return syscall(SYS_pidfd_open, pid, flags); }
static int pidfd_send_signal(int fd, int sig) { return syscall(SYS_pidfd_send_signal, fd, sig, NULL, 0); }

static void fork_wait(void) {
  pid_t parent = getpid();
  int p[2];
  CHECK(pipe(p) == 0, "pipe");
  pid_t pid = fork();
  CHECK(pid >= 0, "fork");
  if (pid == 0) {
    close(p[0]);
    pid_t me[3] = {getpid(), getppid(), gettid()};
    write(p[1], me, sizeof(me));
    _exit(7);
  }
  close(p[1]);
  pid_t me[3];
  CHECK(read(p[0], me, sizeof(me)) == sizeof(me), "child ids");
  CHECK(me[0] == pid && me[1] == parent && me[2] == pid, "child %d/%d/%d, fork %d, parent %d",
        me[0], me[1], me[2], pid, parent);
  CHECK(getpid() == parent, "parent pid changed");
  int st;
  struct rusage ru;
  CHECK(wait4(pid, &st, 0, &ru) == pid, "wait4");
  CHECK(WIFEXITED(st) && WEXITSTATUS(st) == 7, "status %#x", st);
  CHECK(wait4(-1, &st, WNOHANG, NULL) == -1 && errno == ECHILD, "no children left");
  printf("ok fork_wait\n");
}

static void pipe_echo(void) {
  int down[2], up[2];
  CHECK(pipe(down) == 0 && pipe(up) == 0, "pipes");
  pid_t pid = fork();
  if (pid == 0) {
    close(down[1]);
    close(up[0]);
    char buf[64];
    ssize_t n;
    while ((n = read(down[0], buf, sizeof(buf))) > 0) {
      for (ssize_t i = 0; i < n; i++) buf[i] ^= 0x20;
      write(up[1], buf, n);
    }
    _exit(n == 0 ? 0 : 2);
  }
  close(down[0]);
  close(up[1]);
  CHECK(write(down[1], "hello", 5) == 5, "write");
  close(down[1]);
  char buf[16] = {0};
  CHECK(read(up[0], buf, sizeof(buf)) == 5 && strcmp(buf, "HELLO") == 0, "echo '%s'", buf);
  int st;
  CHECK(waitpid(pid, &st, 0) == pid && WIFEXITED(st) && WEXITSTATUS(st) == 0, "status %#x", st);
  printf("ok pipe_echo\n");
}

// Run argv with stdout into a pipe; returns what it printed and its status.
static int run_capture(int use_vfork, const char* path, char* const argv[], char* out, size_t len) {
  int p[2];
  CHECK(pipe(p) == 0, "pipe");
  pid_t pid = use_vfork ? vfork() : fork();
  CHECK(pid >= 0, "fork");
  if (pid == 0) {
    dup2(p[1], 1);
    close(p[0]);
    close(p[1]);
    char* envp[] = {"CHECK_ENV=yes", NULL};
    execve(path, argv, envp);
    _exit(127);
  }
  close(p[1]);
  size_t got = 0;
  ssize_t n;
  while (got + 1 < len && (n = read(p[0], out + got, len - 1 - got)) > 0) got += n;
  out[got] = 0;
  close(p[0]);
  int st;
  CHECK(waitpid(pid, &st, 0) == pid, "waitpid");
  return st;
}

static void exec_image(void) {
  char out[4096];
  char* argv[] = {"linker64-renamed", NULL};
  int st = run_capture(1, "/system/bin/linker64", argv, out, sizeof(out));
  CHECK(WIFEXITED(st) && strstr(out, "Usage: linker64-renamed") != NULL, "status %#x, output '%s'",
        st, out);
  printf("ok exec_image\n");
}

static void exec_argv(void) {
  char out[4096];
  char* argv[] = {"zeroth", "one", "two words", NULL};
  int st = run_capture(0, self_path, argv, out, sizeof(out));
  char want[512];
  snprintf(want, sizeof(want), "argv zeroth|one|two words|\nexecfn %s\nenv CHECK_ENV=yes\n", self_path);
  CHECK(WIFEXITED(st) && WEXITSTATUS(st) == 0 && strcmp(out, want) == 0, "got '%s'", out);

  // Empty argv: Linux supplies an empty argv[0].
  char* none[] = {NULL};
  st = run_capture(0, self_path, none, out, sizeof(out));
  CHECK(WIFEXITED(st) && WEXITSTATUS(st) == 3, "empty argv: status %#x '%s'", st, out);

  CHECK(execve("/data/local/tmp/missing", argv, NULL) == -1 && errno == ENOENT, "missing");
  CHECK(execve("/data/local/tmp", argv, NULL) == -1 && errno == EACCES, "directory");
  printf("ok exec_argv\n");
}

static void exec_script(void) {
  const char* script = "/data/local/tmp/script.sh";
  int fd = open(script, O_WRONLY | O_CREAT | O_TRUNC, 0755);
  CHECK(fd >= 0, "create script");
  char line[256];
  int n = snprintf(line, sizeof(line), "#! %s  argv  opt arg \nexit 1\n", self_path);
  CHECK(write(fd, line, n) == n, "write script");
  close(fd);
  char out[4096];
  char* argv[] = {"ignored", "extra", NULL};
  int st = run_capture(0, script, argv, out, sizeof(out));
  char want[512];
  snprintf(want, sizeof(want), "argv %s|argv  opt arg|%s|extra|\nexecfn %s\nenv CHECK_ENV=yes\n",
           self_path, script, script);
  CHECK(WIFEXITED(st) && WEXITSTATUS(st) == 0 && strcmp(out, want) == 0, "got '%s'", out);

  fd = open(script, O_WRONLY | O_TRUNC);
  write(fd, "plain text\n", 11);
  close(fd);
  CHECK(execve(script, argv, NULL) == -1 && errno == ENOEXEC, "not executable format");
  unlink(script);
  printf("ok exec_script\n");
}

static void waitid_variants(void) {
  pid_t pid = fork();
  if (pid == 0) {
    for (;;) sleep(100);
  }
  siginfo_t si;
  memset(&si, 0xff, sizeof(si));
  CHECK(waitid(P_PID, pid, &si, WEXITED | WNOHANG) == 0 && si.si_pid == 0, "running: pid %d",
        si.si_pid);
  int fd = pidfd_open(pid, 0);
  CHECK(fd >= 0, "pidfd_open");

  CHECK(pidfd_send_signal(fd, SIGSTOP) == 0, "SIGSTOP");
  CHECK(waitid(P_PID, pid, &si, WSTOPPED) == 0, "stopped");
  CHECK(si.si_signo == SIGCHLD && si.si_code == CLD_STOPPED && si.si_status == SIGSTOP &&
            si.si_pid == pid,
        "stop: signo %d code %d status %d", si.si_signo, si.si_code, si.si_status);
  CHECK(pidfd_send_signal(fd, SIGCONT) == 0, "SIGCONT");
  CHECK(waitid(P_ALL, 0, &si, WCONTINUED) == 0 && si.si_code == CLD_CONTINUED, "continued: code %d",
        si.si_code);

  CHECK(pidfd_send_signal(fd, SIGTERM) == 0, "SIGTERM");
  CHECK(waitid(P_PIDFD, fd, &si, WEXITED | WNOWAIT) == 0, "nowait");
  CHECK(si.si_code == CLD_KILLED && si.si_status == SIGTERM, "killed: code %d status %d",
        si.si_code, si.si_status);
  struct rusage ru;
  memset(&ru, 0xff, sizeof(ru));
  CHECK(syscall(SYS_waitid, P_PGID, 0, &si, WEXITED, &ru) == 0 && si.si_pid == pid, "reap");
  CHECK(ru.ru_utime.tv_usec < 1000000 && ru.ru_maxrss >= 0, "rusage");
  CHECK(waitid(P_ALL, 0, &si, WEXITED) == -1 && errno == ECHILD, "no more children");
  CHECK(waitid(P_ALL, 0, &si, WNOHANG) == -1 && errno == EINVAL, "no event type");
  close(fd);
  printf("ok waitid_variants\n");
}

static void pidfd_poll(void) {
  int go[2];
  CHECK(pipe(go) == 0, "pipe");
  pid_t pid = fork();
  if (pid == 0) {
    char c;
    close(go[1]);
    read(go[0], &c, 1);
    _exit(5);
  }
  close(go[0]);
  int fd = pidfd_open(pid, PIDFD_NONBLOCK);
  CHECK(fd >= 0, "pidfd_open");
  CHECK((fcntl(fd, F_GETFD) & FD_CLOEXEC) != 0, "pidfds close on exec");
  struct pollfd pfd = {fd, POLLIN, 0};
  CHECK(poll(&pfd, 1, 0) == 0, "readable before exit");
  // A forked process inherits the pidfd.
  pid_t other = fork();
  if (other == 0) _exit(fcntl(fd, F_GETFD) >= 0 && pidfd_send_signal(fd, 0) == 0 ? 0 : 1);
  int st;
  CHECK(waitpid(other, &st, 0) == other && WIFEXITED(st) && WEXITSTATUS(st) == 0, "inherited");
  siginfo_t si;
  CHECK(waitid(P_PIDFD, fd, &si, WEXITED) == -1 && errno == EAGAIN, "nonblocking pidfd");
  write(go[1], "x", 1);
  CHECK(poll(&pfd, 1, 5000) == 1 && (pfd.revents & POLLIN), "readable after exit");
  CHECK(waitid(P_PIDFD, fd, &si, WEXITED) == 0 && si.si_status == 5, "status %d", si.si_status);
  CHECK(pidfd_send_signal(fd, 0) == -1 && errno == ESRCH, "reaped");
  close(fd);
  CHECK(pidfd_send_signal(fd, 0) == -1 && errno == EBADF, "closed pidfd");
  CHECK(pidfd_open(getpid(), 0x1234) == -1 && errno == EINVAL, "flags");
  printf("ok pidfd_poll\n");
}

// A forked child inherits an epoll fd with its registrations.
static void epoll_fork(void) {
  int p[2];
  CHECK(pipe(p) == 0, "pipe");
  int ep = epoll_create1(EPOLL_CLOEXEC);
  struct epoll_event ev = {.events = EPOLLIN, .data.u64 = 0x1234};
  CHECK(ep >= 0 && epoll_ctl(ep, EPOLL_CTL_ADD, p[0], &ev) == 0, "epoll_ctl");
  pid_t pid = fork();
  if (pid == 0) {
    struct epoll_event got;
    int n = epoll_wait(ep, &got, 1, 5000);
    _exit(n == 1 && got.data.u64 == 0x1234 && (fcntl(ep, F_GETFD) & FD_CLOEXEC) ? 0 : 1);
  }
  CHECK(write(p[1], "x", 1) == 1, "write");
  int st;
  CHECK(waitpid(pid, &st, 0) == pid && WIFEXITED(st) && WEXITSTATUS(st) == 0, "child %#x", st);
  printf("ok epoll_fork\n");
}

static void death_by_signal(void) {
  pid_t pid = fork();
  if (pid == 0) {
    *(volatile int*)16 = 1;
    _exit(0);
  }
  int st;
  CHECK(waitpid(pid, &st, 0) == pid, "waitpid");
  CHECK(WIFSIGNALED(st) && (WTERMSIG(st) == SIGSEGV || WTERMSIG(st) == SIGBUS), "status %#x", st);
  printf("ok death_by_signal\n");
}

static void identity(void) {
  struct __user_cap_header_struct h = {_LINUX_CAPABILITY_VERSION_3, 0};
  struct __user_cap_data_struct d[2];
  CHECK(getuid() == 0 && geteuid() == 0 && getgid() == 0, "starts as root");
  CHECK(capget(&h, d) == 0 && d[0].effective == 0xffffffff && d[1].effective == 0x1ff,
        "root caps %#x %#x", d[0].effective, d[1].effective);
  CHECK(prctl(PR_CAPBSET_READ, 40) == 1 && prctl(PR_CAPBSET_READ, 41) == -1, "bounding set");
  gid_t groups[] = {3003, 1065};
  CHECK(setgroups(2, groups) == 0 && getgroups(0, NULL) == 2, "groups");
  pid_t pid = fork();
  if (pid == 0) {
    // A child inherits the identity, then drops to an app uid.
    CHECK(getgroups(0, NULL) == 2, "inherited groups");
    CHECK(setresgid(10057, 10057, 10057) == 0, "setresgid");
    CHECK(setresuid(10057, 10057, 10057) == 0, "setresuid");
    uid_t r, e, s;
    CHECK(getresuid(&r, &e, &s) == 0 && r == 10057 && e == 10057 && s == 10057, "resuid");
    CHECK(capget(&h, d) == 0 && d[0].effective == 0 && d[0].permitted == 0, "caps dropped");
    CHECK(setuid(0) == -1 && errno == EPERM, "cannot regain root");
    CHECK(setgroups(0, NULL) == -1 && errno == EPERM, "setgroups needs CAP_SETGID");
    // exec keeps the ids.
    execve(self_path, (char*[]){"ids", NULL}, NULL);
    _exit(9);
  }
  int st;
  CHECK(waitpid(pid, &st, 0) == pid && WIFEXITED(st) && WEXITSTATUS(st) == 0, "child %#x", st);
  CHECK(getuid() == 0, "parent unchanged");
  struct utsname u;
  CHECK(uname(&u) == 0 && strcmp(u.sysname, "Linux") == 0 && strstr(u.release, "-android"),
        "uname %s", u.release);
  struct rlimit rl;
  CHECK(getrlimit(RLIMIT_NICE, &rl) == 0, "getrlimit");
  rl.rlim_cur = rl.rlim_max = 30;
  CHECK(setrlimit(RLIMIT_NICE, &rl) == 0 && getrlimit(RLIMIT_NICE, &rl) == 0 && rl.rlim_cur == 30,
        "setrlimit");
  CHECK(umask(022) >= 0 && umask(077) == 022, "umask");
  printf("ok identity\n");
}

// Started by tests/process.rs with --identity (uid 1000, gid 1001, groups
// 3003 1065, CAP_SETGID, CAP_SETUID, CAP_NET_ADMIN and CAP_NET_RAW,
// priority 10, RLIMIT_NICE 40),
// --inherit-env and fd 3 open.
static void identity_file(void) {
  struct __user_cap_header_struct h = {_LINUX_CAPABILITY_VERSION_3, 0};
  struct __user_cap_data_struct d[2];
  gid_t groups[4];
  CHECK(getuid() == 1000 && geteuid() == 1000 && getgid() == 1001, "ids %d %d", getuid(), getgid());
  CHECK(getgroups(4, groups) == 2 && groups[0] == 3003 && groups[1] == 1065, "groups");
  CHECK(capget(&h, d) == 0 && d[0].effective == 0x30c0 && d[0].permitted == 0x30c0, "caps %#x",
        d[0].effective);
  CHECK(prctl(PR_CAP_AMBIENT, PR_CAP_AMBIENT_IS_SET, 12, 0, 0) == 1, "ambient");
  struct rlimit rl;
  CHECK(getrlimit(RLIMIT_NICE, &rl) == 0 && rl.rlim_cur == 40 && rl.rlim_max == 40, "rlimit");
  CHECK(getpriority(PRIO_PROCESS, 0) == 10, "priority %d", getpriority(PRIO_PROCESS, 0));
  CHECK(getenv("ANDROID_SOCKET_test") && strcmp(getenv("ANDROID_SOCKET_test"), "3") == 0, "env");
  CHECK(fcntl(3, F_GETFD) == 0, "inherited fd 3 open without FD_CLOEXEC");

  int go[2], ready[2];
  CHECK(pipe(go) == 0 && pipe(ready) == 0, "pipes");
  pid_t pid = fork();
  if (pid == 0) {
    CHECK(setresuid(10057, 10057, 10057) == 0, "setresuid");
    write(ready[1], "r", 1);
    char c;
    read(go[0], &c, 1);
    _exit(0);
  }
  close(ready[1]);
  close(go[0]);
  char c, path[128], text[1024] = {0};
  CHECK(read(ready[0], &c, 1) == 1, "child ready");
  snprintf(path, sizeof(path), "/data/local/tmp/id/by-pid/%d", pid);
  int fd = open(path, O_RDONLY);
  CHECK(fd >= 0, "%s", path);
  read(fd, text, sizeof(text) - 1);
  close(fd);
  CHECK(strstr(text, "\nuid\t10057\n") && strstr(text, "\ngroups\t3003 1065\n"), "entry '%s'", text);
  write(go[1], "g", 1);
  int st;
  CHECK(waitpid(pid, &st, 0) == pid && WIFEXITED(st), "child");
  CHECK(access(path, F_OK) == -1 && errno == ENOENT, "entry removed at exit");
  printf("ok identity_file\n");
}

static double now_us(void) {
  struct timespec ts;
  clock_gettime(CLOCK_MONOTONIC, &ts);
  return ts.tv_sec * 1e6 + ts.tv_nsec / 1e3;
}

static int cmp(const void* a, const void* b) {
  double x = *(const double*)a, y = *(const double*)b;
  return x < y ? -1 : x > y;
}

// fork+exit+wait and fork+exec+wait latency (p50 and p90 over N runs).
static void bench(void) {
  enum { N = 50 };
  double t[N];
  for (int i = 0; i < N; i++) {
    double s = now_us();
    pid_t pid = fork();
    if (pid == 0) _exit(0);
    waitpid(pid, NULL, 0);
    t[i] = now_us() - s;
  }
  qsort(t, N, sizeof(double), cmp);
  printf("bench fork+exit+wait p50 %.0f us p90 %.0f us\n", t[N / 2], t[N * 9 / 10]);
  for (int i = 0; i < N / 5; i++) {
    double s = now_us();
    pid_t pid = fork();
    if (pid == 0) {
      execve(self_path, (char*[]){"exit", NULL}, NULL);
      _exit(127);
    }
    int st;
    waitpid(pid, &st, 0);
    CHECK(WIFEXITED(st) && WEXITSTATUS(st) == 0, "exec status %#x", st);
    t[i] = now_us() - s;
  }
  qsort(t, N / 5, sizeof(double), cmp);
  printf("bench fork+exec+exit+wait p50 %.0f us p90 %.0f us\n", t[N / 10], t[N / 5 * 9 / 10]);
}

static void seccomp_filter(void) {
  // What minijail does for mediaextractor and media.swcodec.
  struct sock_filter allow = BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW);
  struct sock_fprog prog = {1, &allow};
  CHECK(prctl(PR_GET_SECCOMP) == 0, "no filter yet");
  CHECK(prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) == 0, "no_new_privs");
  CHECK(prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &prog) == 0, "filter");
  CHECK(prctl(PR_GET_SECCOMP) == SECCOMP_MODE_FILTER, "mode");
  printf("ok seccomp_filter\n");
}

int main(int argc, char** argv) {
  self_path = "/data/local/tmp/process";
  // An empty argv arrives as one empty argv[0].
  if (argc == 1 && argv[0][0] == 0) return 3;
  if (strcmp(argv[0], "exit") == 0) return 0;
  if (strcmp(argv[0], "ids") == 0) {
    struct __user_cap_header_struct h = {_LINUX_CAPABILITY_VERSION_3, 0};
    struct __user_cap_data_struct d[2];
    return getuid() == 10057 && getegid() == 10057 && getgroups(0, NULL) == 2 &&
                   capget(&h, d) == 0 && d[0].effective == 0
               ? 0
               : 1;
  }
  // As exec target and as a script's interpreter (argv[1] is the #! argument).
  if (strcmp(argv[0], "zeroth") == 0 || (argc >= 2 && strcmp(argv[1], "argv  opt arg") == 0)) {
    printf("argv ");
    for (int i = 0; i < argc; i++) printf("%s|", argv[i]);
    printf("\nexecfn %s\n", (const char*)getauxval(AT_EXECFN));
    printf("env %s\n", getenv("CHECK_ENV") ? "CHECK_ENV=yes" : "missing");
    return 0;
  }
  const char* which = argc > 1 ? argv[1] : "all";
  struct {
    const char* name;
    void (*fn)(void);
  } checks[] = {
      {"fork_wait", fork_wait},     {"pipe_echo", pipe_echo},
      {"exec_image", exec_image},   {"exec_argv", exec_argv},
      {"exec_script", exec_script}, {"waitid_variants", waitid_variants},
      {"pidfd_poll", pidfd_poll},   {"epoll_fork", epoll_fork},   {"death_by_signal", death_by_signal},
      {"identity", identity},       {"identity_file", identity_file},
      {"seccomp_filter", seccomp_filter},
      {"bench", bench},
  };
  for (size_t i = 0; i < sizeof(checks) / sizeof(checks[0]); i++) {
    if (strcmp(which, checks[i].name) == 0 ||
        (strcmp(which, "all") == 0 && checks[i].fn != bench && checks[i].fn != identity_file)) {
      fflush(stdout);
      checks[i].fn();
      fflush(stdout);
    }
  }
  return 0;
}
