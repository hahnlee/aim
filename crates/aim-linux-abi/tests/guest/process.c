// Process lifecycle checks run as a guest under linux-run (tests/process.rs).
// Built with the NDK for aarch64-linux-android; every check prints one
// "ok <name>" line or fails with a message and exit status 1.
//
// usage: process CHECK
//        process argv ARGS...   (prints its argv and AT_EXECFN; used as a
//                                script interpreter and exec target)

#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <linux/filter.h>
#include <linux/seccomp.h>
#include <fcntl.h>
#include <poll.h>
#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/auxv.h>
#include <sys/capability.h>
#include <sys/epoll.h>
#include <sys/eventfd.h>
#include <sys/inotify.h>
#include <sys/timerfd.h>
#include <sys/mman.h>
#include <sys/mount.h>
#include <sched.h>
#include <sys/prctl.h>
#include <sys/resource.h>
#include <linux/futex.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/sysinfo.h>
#include <sys/uio.h>
#include <sys/utsname.h>
#include <sys/wait.h>
#include <sys/xattr.h>
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
// main's argv, and the end of its strings.
static char** main_argv;
static char* main_args_end;

static int pidfd_open(pid_t pid, unsigned flags) { return syscall(SYS_pidfd_open, pid, flags); }
static int pidfd_send_signal(int fd, int sig) { return syscall(SYS_pidfd_send_signal, fd, sig, NULL, 0); }

// Code that becomes executable in a forked child is rewritten there, and
// its syscall stubs may go into a trampoline island the parent made before
// the fork: the child must run what it wrote.
static void fork_new_code(void) {
  // getpid(): mov x8, #172; svc #0; ret
  static const unsigned code[] = {0xd2801588, 0xd4000001, 0xd65f03c0};
  // Rewriting in the parent first creates the island the child reuses.
  void* first = mmap(NULL, 16384, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  CHECK(first != MAP_FAILED, "mmap");
  memcpy(first, code, sizeof(code));
  CHECK(mprotect(first, 16384, PROT_READ | PROT_EXEC) == 0, "mprotect");
  CHECK(((pid_t(*)(void))first)() == getpid(), "parent code");
  pid_t pid = fork();
  CHECK(pid >= 0, "fork");
  if (pid == 0) {
    void* p = mmap(NULL, 16384, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (p == MAP_FAILED) _exit(2);
    memcpy(p, code, sizeof(code));
    if (mprotect(p, 16384, PROT_READ | PROT_EXEC) != 0) _exit(3);
    _exit(((pid_t(*)(void))p)() == getpid() ? 0 : 4);
  }
  int st;
  CHECK(waitpid(pid, &st, 0) == pid, "waitpid");
  CHECK(WIFEXITED(st) && WEXITSTATUS(st) == 0, "child status %#x", st);
  printf("ok fork_new_code\n");
}

static int mounted(const char* target) {
  char buf[8192] = {0};
  int fd = open("/proc/self/mounts", O_RDONLY);
  if (fd < 0) return -1;
  read(fd, buf, sizeof(buf) - 1);
  close(fd);
  return strstr(buf, target) != NULL;
}

// zygote's storage and app data views: a private mount namespace with bind
// and tmpfs mounts, inherited by a forked child and undone by umount.
static void mount_ns(void) {
  const char* src = "/data/local/tmp/ns-src";
  const char* dst = "/data/local/tmp/ns-dst";
  mkdir(src, 0755);
  mkdir(dst, 0755);
  int fd = open("/data/local/tmp/ns-src/f", O_CREAT | O_WRONLY | O_TRUNC, 0644);
  CHECK(fd >= 0, "create");
  close(fd);
  CHECK(unshare(CLONE_NEWNS) == 0, "unshare");
  CHECK(mount("rootfs", "/", NULL, MS_SLAVE | MS_REC, NULL) == 0, "propagation");
  CHECK(mount(src, dst, NULL, MS_BIND | MS_REC, NULL) == 0, "bind");
  CHECK(access("/data/local/tmp/ns-dst/f", F_OK) == 0, "bound file");
  CHECK(mounted(" /data/local/tmp/ns-dst ") == 1, "listed");
  CHECK(mount("tmpfs", src, "tmpfs", MS_NOSUID, "mode=0751") == 0, "tmpfs");
  CHECK(access("/data/local/tmp/ns-src/f", F_OK) != 0, "tmpfs hides the directory");
  pid_t pid = fork();
  CHECK(pid >= 0, "fork");
  if (pid == 0) _exit(access("/data/local/tmp/ns-dst/f", F_OK) == 0 ? 0 : 1);
  int st;
  CHECK(waitpid(pid, &st, 0) == pid && WIFEXITED(st) && WEXITSTATUS(st) == 0, "child view");
  CHECK(umount2(src, MNT_DETACH) == 0 && umount2(dst, 0) == 0, "umount");
  CHECK(umount2(dst, 0) == -1 && errno == EINVAL, "not a mount");
  CHECK(access("/data/local/tmp/ns-dst/f", F_OK) != 0, "unbound");
  CHECK(mount("none", dst, "sdcardfs", 0, NULL) == -1 && errno == ENODEV, "unknown fs");
  CHECK(unshare(CLONE_NEWNET) == -1 && errno == EINVAL, "no network namespaces");
  printf("ok mount_ns\n");
}

// installd's restorecon: every file has a SELinux label (unlabeled until
// set), and guest attributes round-trip without the host's showing.
static void xattrs(void) {
  const char* p = "/data/local/tmp/xattr-f";
  int fd = open(p, O_CREAT | O_WRONLY | O_TRUNC, 0644);
  CHECK(fd >= 0, "create");
  char buf[128];
  ssize_t n = lgetxattr(p, "security.selinux", buf, sizeof(buf));
  CHECK(n > 0 && strcmp(buf, "u:object_r:unlabeled:s0") == 0, "unlabeled");
  const char* label = "u:object_r:app_data_file:s0";
  CHECK(lsetxattr(p, "security.selinux", label, strlen(label) + 1, 0) == 0, "set label");
  n = getxattr(p, "security.selinux", buf, sizeof(buf));
  CHECK(n == (ssize_t)strlen(label) + 1 && strcmp(buf, label) == 0, "label");
  CHECK(getxattr(p, "security.selinux", buf, 4) == -1 && errno == ERANGE, "too small");
  CHECK(fsetxattr(fd, "user.k", "v", 1, XATTR_CREATE) == 0, "fset");
  CHECK(fsetxattr(fd, "user.k", "w", 1, XATTR_CREATE) == -1 && errno == EEXIST, "create twice");
  CHECK(fgetxattr(fd, "user.k", buf, sizeof(buf)) == 1 && buf[0] == 'v', "fget");
  n = listxattr(p, buf, sizeof(buf));
  CHECK(n == (ssize_t)sizeof("security.selinux\0user.k"), "list");
  CHECK(removexattr(p, "user.k") == 0, "remove");
  CHECK(getxattr(p, "user.k", buf, sizeof(buf)) == -1 && errno == ENODATA, "removed");
  CHECK(getxattr("/data/local/tmp/absent", "security.selinux", buf, sizeof(buf)) == -1 &&
            errno == ENOENT,
        "absent file");
  close(fd);
  unlink(p);
  printf("ok xattrs\n");
}

// libbase's SendFileDescriptors with no fds sends a bare SCM_RIGHTS
// header; Linux delivers the data without a control message.
static void empty_rights(void) {
  int sv[2];
  CHECK(socketpair(AF_UNIX, SOCK_SEQPACKET, 0, sv) == 0, "socketpair");
  char cbuf[CMSG_SPACE(sizeof(int))] = {0};
  char byte = 'x';
  struct iovec iov = {&byte, 1};
  struct msghdr m = {0};
  m.msg_iov = &iov;
  m.msg_iovlen = 1;
  m.msg_control = cbuf;
  m.msg_controllen = CMSG_SPACE(0);
  struct cmsghdr* c = CMSG_FIRSTHDR(&m);
  c->cmsg_level = SOL_SOCKET;
  c->cmsg_type = SCM_RIGHTS;
  c->cmsg_len = CMSG_LEN(0);
  CHECK(sendmsg(sv[0], &m, 0) == 1, "send");
  memset(cbuf, 0, sizeof(cbuf));
  byte = 0;
  m.msg_controllen = sizeof(cbuf);
  CHECK(recvmsg(sv[1], &m, 0) == 1 && byte == 'x', "recv");
  CHECK(m.msg_controllen == 0 && CMSG_FIRSTHDR(&m) == NULL, "no control message");
  close(sv[0]);
  close(sv[1]);
  printf("ok empty_rights\n");
}

// bionic's debuggerd handler: a "pseudothread" with its own file table
// (CLONE_THREAD without CLONE_FILES) closes every fd, opens /dev/null as
// fd 0; the spawner waits on the tid word and keeps its own fds.
static volatile pid_t pseudo_tid = -1;
static int pseudo_fn(void* arg) {
  (void)arg;
  for (int i = 0; i < 1024; i++) syscall(__NR_close, i);
  int fd = open("/dev/null", O_RDWR);
  _exit(fd == 0 ? 0 : 1);
}
static void own_files_thread(void) {
  int p[2];
  CHECK(pipe(p) == 0, "pipe");
  static char stack[64 * 1024];
  pid_t tid = clone(pseudo_fn, stack + sizeof(stack),
                    CLONE_THREAD | CLONE_SIGHAND | CLONE_VM | CLONE_CHILD_SETTID |
                        CLONE_CHILD_CLEARTID,
                    NULL, NULL, NULL, (pid_t*)&pseudo_tid);
  CHECK(tid > 0, "clone");
  while (pseudo_tid == -1) syscall(__NR_futex, &pseudo_tid, FUTEX_WAIT, -1, NULL, NULL, 0);
  while (pseudo_tid != 0) syscall(__NR_futex, &pseudo_tid, FUTEX_WAIT, pseudo_tid, NULL, NULL, 0);
  CHECK(write(p[1], "x", 1) == 1, "the spawner's fds stay open");
  char c;
  CHECK(read(p[0], &c, 1) == 1 && c == 'x', "read back");
  close(p[0]);
  close(p[1]);
  printf("ok own_files_thread\n");
}

// libbpf_android's synchronizeKernelRCU: a PF_KEY socket opens and closes.
static void pf_key(void) {
  int fd = socket(15 /* AF_KEY */, SOCK_RAW | SOCK_CLOEXEC, 2 /* PF_KEY_V2 */);
  CHECK(fd >= 0, "socket");
  CHECK(close(fd) == 0, "close");
  CHECK(socket(15, SOCK_DGRAM, 2) == -1 && errno == EAFNOSUPPORT, "only raw v2");
  printf("ok pf_key\n");
}

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

// After fork, private memory is each process's own copy and shared memory
// is one: writes to a private page after the fork stay in the process that
// made them, in both directions, while a MAP_SHARED page and a memfd
// mapping carry them across. Descriptors (with their close-on-exec flags),
// an eventfd watched by epoll, an inotify watch and an armed timerfd come
// along.
static void fork_memory(void) {
  enum { PAGE = 16384 };
  char* priv = mmap(NULL, 4 * PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  char* shared = mmap(NULL, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANONYMOUS, -1, 0);
  int mfd = memfd_create("fork_memory", MFD_CLOEXEC);
  CHECK(mfd >= 0 && ftruncate(mfd, PAGE) == 0, "memfd");
  char* viamfd = mmap(NULL, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, mfd, 0);
  // A page protected to nothing keeps its contents.
  char* hidden = mmap(NULL, PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  CHECK(priv != MAP_FAILED && shared != MAP_FAILED && viamfd != MAP_FAILED && hidden != MAP_FAILED,
        "mmap");
  strcpy(priv, "before");
  strcpy(priv + 3 * PAGE, "far");
  strcpy(shared, "before");
  strcpy(viamfd, "before");
  strcpy(hidden, "hidden");
  CHECK(mprotect(hidden, PAGE, PROT_NONE) == 0, "mprotect");
  static int in_data = 1;
  int cloexec_fd = open("/data/local/tmp/process", O_RDONLY | O_CLOEXEC);
  int plain_fd = open("/data/local/tmp/process", O_RDONLY);
  int efd = eventfd(0, EFD_CLOEXEC);
  int ep = epoll_create1(EPOLL_CLOEXEC);
  struct epoll_event ev = {.events = EPOLLIN, .data.u64 = 0xfeed};
  CHECK(cloexec_fd >= 0 && plain_fd >= 0 && efd >= 0 && ep >= 0, "fds");
  CHECK(epoll_ctl(ep, EPOLL_CTL_ADD, efd, &ev) == 0, "epoll_ctl");
  mkdir("/data/local/tmp/watched", 0700);
  unlink("/data/local/tmp/watched/new");
  int in = inotify_init1(IN_CLOEXEC);
  CHECK(in >= 0 && inotify_add_watch(in, "/data/local/tmp/watched", IN_CREATE) >= 0, "inotify");
  int tfd = timerfd_create(CLOCK_MONOTONIC, TFD_CLOEXEC);
  struct itimerspec its = {.it_value = {.tv_nsec = 50 * 1000 * 1000}};
  CHECK(tfd >= 0 && timerfd_settime(tfd, 0, &its, NULL) == 0, "timerfd");
  int go[2], back[2];
  CHECK(pipe(go) == 0 && pipe(back) == 0, "pipes");
  pid_t pid = fork();
  CHECK(pid >= 0, "fork");
  if (pid == 0) {
    char c;
    int bad = 0;
    bad |= strcmp(priv, "before") || strcmp(priv + 3 * PAGE, "far") || in_data != 1;
    bad |= (mprotect(hidden, PAGE, PROT_READ) != 0 || strcmp(hidden, "hidden")) << 1;
    bad |= !((fcntl(cloexec_fd, F_GETFD) & FD_CLOEXEC) && fcntl(plain_fd, F_GETFD) == 0) << 2;
    // The parent writes, then lets us look.
    if (read(go[0], &c, 1) != 1) _exit(99);
    bad |= (strcmp(priv, "before") || in_data != 1) << 3;
    bad |= (strcmp(shared, "parent") || strcmp(viamfd, "parent")) << 4;
    strcpy(priv, "child");
    in_data = 3;
    strcpy(shared, "child");
    strcpy(viamfd, "child");
    uint64_t one = 1;
    bad |= (write(efd, &one, 8) != 8) << 6;
    struct epoll_event got;
    bad |= (epoll_wait(ep, &got, 1, 5000) != 1 || got.data.u64 != 0xfeed) << 6;
    close(open("/data/local/tmp/watched/new", O_CREAT | O_WRONLY, 0600));
    char buf[256];
    struct pollfd pin = {.fd = in, .events = POLLIN};
    bad |= (poll(&pin, 1, 5000) != 1 || read(in, buf, sizeof(buf)) <= 0 ||
            ((struct inotify_event*)buf)->mask != IN_CREATE) << 5;
    uint64_t ticks = 0;
    bad |= (read(tfd, &ticks, 8) != 8 || ticks != 1) << 7;
    if (write(back[1], "y", 1) != 1) _exit(98);
    _exit(bad);
  }
  strcpy(priv, "parent");
  in_data = 2;
  strcpy(shared, "parent");
  strcpy(viamfd, "parent");
  CHECK(write(go[1], "x", 1) == 1, "go");
  char c;
  CHECK(read(back[0], &c, 1) == 1, "back");
  int st;
  CHECK(waitpid(pid, &st, 0) == pid && WIFEXITED(st), "child %#x", st);
  CHECK(WEXITSTATUS(st) == 0, "child checks failed: mask %#x", WEXITSTATUS(st));
  CHECK(strcmp(priv, "parent") == 0 && in_data == 2, "the child's private writes stay its own");
  CHECK(strcmp(shared, "child") == 0 && strcmp(viamfd, "child") == 0, "shared writes arrive");
  uint64_t v = 0;
  CHECK(read(efd, &v, 8) == 8 && v == 1, "the child's eventfd write reaches the parent");
  printf("ok fork_memory\n");
}

// A parent that exits right after fork (daemon()'s double fork): the
// grandchild still starts and runs.
static void fork_then_exit(void) {
  int p[2];
  CHECK(pipe(p) == 0, "pipe");
  pid_t pid = fork();
  CHECK(pid >= 0, "fork");
  if (pid == 0) {
    close(p[0]);
    if (fork() == 0) {
      usleep(100 * 1000);
      _exit(write(p[1], "g", 1) == 1 ? 0 : 1);
    }
    _exit(0);
  }
  close(p[1]);
  int st;
  CHECK(waitpid(pid, &st, 0) == pid && WIFEXITED(st) && WEXITSTATUS(st) == 0, "child %#x", st);
  char c = 0;
  CHECK(read(p[0], &c, 1) == 1 && c == 'g', "the grandchild ran");
  close(p[0]);
  printf("ok fork_then_exit\n");
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
    close(go[1]);  // so that a failed check in the parent ends the wait
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
  // /proc holds the process table's processes (a pid namespace), not the
  // host's: the child and this process, not the test that started it.
  int listed_self = 0, listed_child = 0, listed_other = 0;
  DIR* proc = opendir("/proc");
  CHECK(proc != NULL, "opendir /proc");
  for (struct dirent* e; (e = readdir(proc));) {
    int p = atoi(e->d_name);
    if (p == getpid()) listed_self = 1;
    else if (p == pid) listed_child = 1;
    else if (p > 0) listed_other = 1;
  }
  closedir(proc);
  CHECK(listed_self && listed_child && !listed_other, "/proc lists %d %d %d", listed_self,
        listed_child, listed_other);
  // The host process that started this one is outside: its pid is 0, in
  // /proc/self/stat too.
  CHECK(getppid() == 0, "parent %d", getppid());
  int ppid = -1;
  FILE* stat = fopen("/proc/self/stat", "r");
  CHECK(stat && fscanf(stat, "%*d (%*[^)]) %*c %d", &ppid) == 1 && ppid == 0, "stat ppid %d", ppid);
  fclose(stat);
  snprintf(path, sizeof(path), "/proc/%d/stat", pid);
  CHECK(access(path, F_OK) == 0, "the child is in /proc");
  snprintf(path, sizeof(path), "/data/local/tmp/id/by-pid/%d", pid);
  write(go[1], "g", 1);
  int st;
  CHECK(waitpid(pid, &st, 0) == pid && WIFEXITED(st), "child");
  CHECK(access(path, F_OK) == -1 && errno == ENOENT, "entry removed at exit");
  // Without CAP_SYS_NICE, RLIMIT_NICE bounds how far the nice value drops
  // (SystemUI's wmshell thread takes -4 under init's limit of 40).
  CHECK(setpriority(PRIO_PROCESS, 0, -4) == 0 && getpriority(PRIO_PROCESS, 0) == -4, "nice -4");
  rl.rlim_cur = rl.rlim_max = 25;
  CHECK(setrlimit(RLIMIT_NICE, &rl) == 0, "lower RLIMIT_NICE");
  CHECK(setpriority(PRIO_PROCESS, 0, -10) == -1 && errno == EACCES, "nice -10 beyond the limit");
  CHECK(setpriority(PRIO_PROCESS, 0, -5) == 0, "nice -5 within it");
  printf("ok identity_file\n");
}

// A pid namespace (run with --identity, so the process table is the
// namespace): host process `host`, which leads its own process group, does
// not exist for the guest, and signals to groups or to every process reach
// the table's processes only.
static void pid_namespace(pid_t host) {
  char path[64];
#define GONE(call) CHECK((call) == -1 && errno == ESRCH, "%s", #call)
  GONE(kill(host, 0));
  GONE(kill(host, SIGKILL));
  GONE(kill(-host, SIGKILL));
  GONE(syscall(SYS_tkill, host, SIGKILL));
  GONE(syscall(SYS_tgkill, host, host, SIGKILL));
  siginfo_t si = {.si_signo = SIGKILL, .si_code = SI_QUEUE};
  GONE(syscall(SYS_rt_sigqueueinfo, host, SIGKILL, &si));
  GONE(pidfd_open(host, 0));
  struct sched_param sp = {0};
  cpu_set_t set;
  GONE(sched_getscheduler(host));
  GONE(sched_setscheduler(host, SCHED_OTHER, &sp));
  GONE(sched_getparam(host, &sp));
  GONE(sched_setparam(host, &sp));
  GONE(sched_getaffinity(host, sizeof(set), &set));
  GONE(sched_setaffinity(host, sizeof(set), &set));
  errno = 0;
  GONE(getpriority(PRIO_PROCESS, host));
  GONE(setpriority(PRIO_PROCESS, host, 19));
  GONE(getpriority(PRIO_PGRP, host));
  GONE(setpriority(PRIO_PGRP, host, 19));
  struct rlimit rl;
  GONE(prlimit(host, RLIMIT_NOFILE, NULL, &rl));
  GONE(getpgid(host));
  GONE(getsid(host));
  GONE(setpgid(host, host));
  char buf[8];
  struct iovec local = {buf, sizeof(buf)}, remote = {buf, sizeof(buf)};
  GONE(process_vm_readv(host, &local, 1, &remote, 1, 0));
  struct __user_cap_header_struct h = {_LINUX_CAPABILITY_VERSION_3, host};
  struct __user_cap_data_struct d[2];
  GONE(capget(&h, d));
  snprintf(path, sizeof(path), "/proc/%d/stat", host);
  CHECK(access(path, F_OK) == -1 && errno == ENOENT, "%s", path);
  // The host process that started this one is its parent and leads its
  // group and session.
  CHECK(getppid() == 0, "outside parent %d", getppid());
  CHECK(getpgid(0) == 0 && getsid(0) == 0, "outside group %d session %d", getpgid(0), getsid(0));
  // Alone in the namespace, kill(-1) has no target (Linux skips the caller)
  // and PRIO_USER names this process only.
  GONE(kill(-1, 0));
  CHECK(setpriority(PRIO_USER, 0, 5) == 0 && getpriority(PRIO_USER, 0) == 5, "PRIO_USER");
  CHECK(getpriority(PRIO_USER, getuid() + 1) == -1 && errno == ESRCH, "another uid");
  CHECK(kill(0, 0) == 0, "own group");
  struct sysinfo sys;
  CHECK(sysinfo(&sys) == 0 && sys.procs == 1, "procs %d alone", sys.procs);

  int ready[2];
  CHECK(pipe(ready) == 0, "pipe");
  pid_t child = fork();
  if (child == 0) {
    setpgid(0, 0);
    write(ready[1], "r", 1);
    for (;;) pause();
  }
  // Right after fork, before the child ran: in the namespace.
  CHECK(kill(child, 0) == 0 && getpgid(child) >= 0, "child");
  char c;
  CHECK(read(ready[0], &c, 1) == 1, "child ready");
  CHECK(getpgid(child) == child && getsid(child) == 0, "child group %d", getpgid(child));
  CHECK(sysinfo(&sys) == 0 && sys.procs == 2, "procs %d with the child", sys.procs);
  int fd = pidfd_open(child, 0);
  CHECK(fd >= 0 && pidfd_send_signal(fd, 0) == 0, "pidfd");
  CHECK(kill(-child, 0) == 0 && kill(-1, 0) == 0, "child group and kill(-1)");
  CHECK(kill(-1, SIGKILL) == 0, "kill(-1) reaches the child");
  int st;
  CHECK(waitpid(child, &st, 0) == child && WIFSIGNALED(st) && WTERMSIG(st) == SIGKILL, "child killed");
  close(fd);
#undef GONE
  printf("ok pid_namespace\n");
}

// A child that takes real, effective and saved uid `r`, `e`, `s` (and the
// same gids), starts a session if asked, reports on `ready` and waits.
static pid_t idle_as(uid_t r, uid_t e, uid_t s, int session, const int ready[2]) {
  pid_t pid = fork();
  CHECK(pid >= 0, "fork");
  if (pid == 0) {
    if (session) setsid();
    if (setresgid(r, e, s) != 0 || setresuid(r, e, s) != 0) _exit(9);
    write(ready[1], "r", 1);
    for (;;) pause();
  }
  char c;
  CHECK(read(ready[0], &c, 1) == 1, "child %d ready", pid);
  return pid;
}

// A child that becomes uid `uid` keeping capabilities `caps` (bits of the
// first word), runs `fn` and exits with its failures.
static void run_as(uid_t uid, uint32_t caps, void (*fn)(void)) {
  pid_t pid = fork();
  CHECK(pid >= 0, "fork");
  if (pid == 0) {
    struct __user_cap_header_struct h = {_LINUX_CAPABILITY_VERSION_3, 0};
    struct __user_cap_data_struct d[2] = {{caps, caps, 0}, {0, 0, 0}};
    CHECK(prctl(PR_SET_KEEPCAPS, 1) == 0, "keepcaps");
    CHECK(setresgid(uid, uid, uid) == 0 && setresuid(uid, uid, uid) == 0, "uid %d", uid);
    CHECK(capset(&h, d) == 0, "capset %#x", caps);
    fn();
    _exit(0);
  }
  int st;
  CHECK(waitpid(pid, &st, 0) == pid && WIFEXITED(st) && WEXITSTATUS(st) == 0, "uid %d: %#x", uid,
        st);
}

// Processes of the permission checks: system uid 1000; an app (10060)
// whose saved uid is 10050; an app 10051 in a session of its own.
static pid_t perm_system, perm_saved, perm_other;

#define DENIED(call) CHECK((call) == -1 && errno == EPERM, "%s", #call)

// An app (uid 10050, no capabilities) against the others.
static void app_permissions(void) {
  struct sched_param sp = {0};
  cpu_set_t set;
  CPU_ZERO(&set);
  CPU_SET(0, &set);
  DENIED(kill(perm_system, 0));
  DENIED(kill(perm_system, SIGKILL));
  DENIED(kill(getppid(), SIGTERM));
  DENIED(syscall(SYS_tgkill, perm_system, perm_system, SIGKILL));
  int fd = pidfd_open(perm_system, 0);
  CHECK(fd >= 0, "pidfd_open needs no permission");
  DENIED(pidfd_send_signal(fd, SIGKILL));
  close(fd);
  DENIED(setpriority(PRIO_PROCESS, perm_system, 19));
  DENIED(sched_setscheduler(perm_system, SCHED_OTHER, &sp));
  DENIED(sched_setparam(perm_system, &sp));
  DENIED(sched_setaffinity(perm_system, sizeof(set), &set));
  CHECK(getpriority(PRIO_PROCESS, perm_system) >= -20 && sched_getscheduler(perm_system) >= 0,
        "reading is allowed");
  // SIGCONT within the session, not outside it.
  CHECK(kill(perm_system, SIGCONT) == 0, "SIGCONT in the session");
  DENIED(kill(perm_other, SIGCONT));
  // The target's saved uid is ours: enough to signal it, but prlimit
  // needs every uid and gid to match.
  CHECK(kill(perm_saved, 0) == 0, "saved uid");
  struct rlimit rl;
  DENIED(prlimit(perm_saved, RLIMIT_NOFILE, NULL, &rl));
  DENIED(prlimit(perm_system, RLIMIT_NOFILE, NULL, &rl));
  // kill(-1) ignores the processes it may not signal.
  CHECK(kill(-1, SIGKILL) == 0, "kill(-1)");
  CHECK(kill(perm_system, 0) == -1 && errno == EPERM, "system survives");
  // Its own processes: signal and renice, but not below their RLIMIT_NICE.
  int ready[2];
  CHECK(pipe(ready) == 0, "pipe");
  pid_t own = fork();
  if (own == 0) {
    struct rlimit rl = {20, 20};
    setrlimit(RLIMIT_NICE, &rl);
    write(ready[1], "r", 1);
    for (;;) pause();
  }
  char c;
  CHECK(read(ready[0], &c, 1) == 1, "own ready");
  CHECK(prlimit(own, RLIMIT_NICE, NULL, &rl) == 0 && rl.rlim_cur == 20 && rl.rlim_max == 20,
        "own process's limit %lu", (unsigned long)rl.rlim_cur);
  CHECK(prlimit(own, RLIMIT_NOFILE, NULL, &rl) == 0 && rl.rlim_cur > 0, "an inherited limit");
  CHECK(syscall(SYS_prlimit64, own, 99, NULL, &rl) == -1 && errno == EINVAL, "bad resource");
  CHECK(setpriority(PRIO_PROCESS, own, 5) == 0, "renice own");
  CHECK(setpriority(PRIO_PROCESS, own, -1) == -1 && errno == EACCES, "below RLIMIT_NICE");
  CHECK(sched_setscheduler(own, SCHED_BATCH, &sp) == 0, "own scheduling");
  CHECK(kill(own, SIGKILL) == 0 && waitpid(own, NULL, 0) == own, "kill own");
}

// uid 1000 with CAP_KILL and CAP_SYS_NICE, as zygote leaves system_server.
static void system_permissions(void) {
  struct sched_param sp = {0};
  CHECK(kill(perm_system, 0) == 0, "same uid");
  CHECK(setpriority(PRIO_PROCESS, perm_other, 10) == 0, "CAP_SYS_NICE renices an app");
  CHECK(sched_setscheduler(perm_other, SCHED_BATCH, &sp) == 0, "and schedules it");
  CHECK(kill(perm_other, 0) == 0 && kill(getppid(), 0) == 0, "CAP_KILL");
  struct __user_cap_header_struct h = {_LINUX_CAPABILITY_VERSION_3, 0};
  struct __user_cap_data_struct d[2] = {{1 << CAP_SYS_NICE, 1 << CAP_SYS_NICE, 0}, {0, 0, 0}};
  CHECK(capset(&h, d) == 0, "drop CAP_KILL");
  DENIED(kill(perm_other, 0));
  struct rlimit rl;
  DENIED(prlimit(perm_other, RLIMIT_NOFILE, NULL, &rl));
  CHECK(kill(perm_system, SIGKILL) == 0, "same uid");
}

static void permissions(void) {
  int ready[2];
  CHECK(pipe(ready) == 0, "pipe");
  perm_system = idle_as(1000, 1000, 1000, 0, ready);
  perm_saved = idle_as(10060, 10060, 10050, 0, ready);
  perm_other = idle_as(10051, 10051, 10051, 1, ready);
  run_as(10050, 0, app_permissions);
  run_as(1000, 1 << CAP_KILL | 1 << CAP_SYS_NICE, system_permissions);
  struct rlimit rl;
  CHECK(prlimit(perm_other, RLIMIT_NOFILE, NULL, &rl) == 0, "CAP_SYS_RESOURCE reads any limit");
  int st;
  CHECK(waitpid(perm_system, &st, 0) == perm_system && WIFSIGNALED(st), "system killed");
  CHECK(kill(perm_saved, SIGKILL) == 0 && kill(perm_other, SIGKILL) == 0, "root kills");
  CHECK(waitpid(perm_saved, NULL, 0) == perm_saved && waitpid(perm_other, NULL, 0) == perm_other,
        "reaped");
  printf("ok permissions\n");
}
#undef DENIED

static double now_us(void) {
  struct timespec ts;
  clock_gettime(CLOCK_MONOTONIC, &ts);
  return ts.tv_sec * 1e6 + ts.tv_nsec / 1e3;
}

static char* slurp(const char* path) {
  static char buf[4096];
  int fd = open(path, O_RDONLY);
  if (fd < 0) return NULL;
  ssize_t n = read(fd, buf, sizeof(buf) - 1);
  close(fd);
  buf[n < 0 ? 0 : n] = 0;
  return buf;
}

static int peer_up[2], peer_down[2];
static volatile pid_t chld_pid;
static volatile uid_t chld_uid;

static void on_chld(int sig, siginfo_t* si, void* uc) {
  (void)sig;
  (void)uc;
  chld_pid = si->si_pid;
  chld_uid = si->si_uid;
}

// A thread of the child: it names itself, reports its tid, and reports
// whether the scheduling the parent set reached it.
static void* peer_worker(void* arg) {
  (void)arg;
  prctl(PR_SET_NAME, "peer-worker");
  pid_t tid = gettid();
  write(peer_up[1], &tid, sizeof(tid));
  char c;
  read(peer_down[0], &c, 1);
  struct sched_param sp = {0};
  int ok = 0;
  for (int i = 0; i < 5000 && !ok; i++) {
    ok = sched_getscheduler(0) == SCHED_FIFO && sched_getparam(0, &sp) == 0 &&
         sp.sched_priority == 1 && getpriority(PRIO_PROCESS, 0) == -10;
    if (!ok) usleep(1000);
  }
  write(peer_up[1], &ok, sizeof(ok));
  read(peer_down[0], &c, 1);
  return NULL;
}

// Another process through /proc and the scheduling calls: a child
// (uid 10057) renames itself as zygote renames its children (PR_SET_NAME
// and argv rewritten in place) and runs a second thread.
static void peers(void) {
  struct sigaction sa = {.sa_sigaction = on_chld, .sa_flags = SA_SIGINFO | SA_RESTART};
  sigaction(SIGCHLD, &sa, NULL);
  CHECK(pipe(peer_up) == 0 && pipe(peer_down) == 0, "pipes");
  pid_t child = fork();
  if (child == 0) {
    if (setresgid(10057, 10057, 10057) != 0 || setresuid(10057, 10057, 10057) != 0) _exit(9);
    prctl(PR_SET_NAME, "peer-main");
    memset(main_argv[0], 0, main_args_end - main_argv[0]);
    strcpy(main_argv[0], "renamed-child");
    pthread_t t;
    if (pthread_create(&t, NULL, peer_worker, NULL) != 0) _exit(8);
    pthread_join(t, NULL);
    _exit(0);
  }
  pid_t tid;
  CHECK(read(peer_up[0], &tid, sizeof(tid)) == sizeof(tid), "worker tid");
  char path[128], *text;
  // Processes at the top of /proc, their threads under task/.
  int listed_child = 0, listed_tid = 0, named = 0;
  DIR* proc = opendir("/proc");
  for (struct dirent* e; (e = readdir(proc));) {
    int p = atoi(e->d_name);
    listed_child |= p == child;
    listed_tid |= p == tid;
    snprintf(path, sizeof(path), "/proc/%d/cmdline", p);
    if (p > 0 && (text = slurp(path)) && !strcmp(text, "renamed-child")) named = p;
  }
  closedir(proc);
  CHECK(listed_child && !listed_tid, "/proc lists the child %d, not its thread %d", listed_child,
        listed_tid);
  CHECK(named == child, "the child by its rewritten argv: %d", named);
  // The child answers at once, not after a reader's timeout (50 ms).
  snprintf(path, sizeof(path), "/proc/%d/cmdline", child);
  double t0 = now_us();
  for (int i = 0; i < 20; i++) CHECK((text = slurp(path)) && !strcmp(text, "renamed-child"), "cmdline");
  double per_read = (now_us() - t0) / 20;
  CHECK(per_read < 20000, "%.0f us per cmdline read", per_read);
  int tasks = 0;
  snprintf(path, sizeof(path), "/proc/%d/task", child);
  DIR* task = opendir(path);
  CHECK(task != NULL, "%s", path);
  for (struct dirent* e; (e = readdir(task));) tasks += atoi(e->d_name) == child || atoi(e->d_name) == tid;
  closedir(task);
  CHECK(tasks == 2, "task/ lists %d of the 2 threads", tasks);
  snprintf(path, sizeof(path), "/proc/%d/task/%d", child, tid);
  CHECK(access(path, F_OK) == 0, "%s", path);
  snprintf(path, sizeof(path), "/proc/%d/task/%d", child, tid + 1);
  CHECK(access(path, F_OK) == -1 && errno == ENOENT, "%s", path);
  snprintf(path, sizeof(path), "/proc/%d/comm", child);
  CHECK((text = slurp(path)) && !strcmp(text, "peer-main\n"), "comm '%s'", text);
  snprintf(path, sizeof(path), "/proc/%d/task/%d/comm", child, tid);
  CHECK((text = slurp(path)) && !strcmp(text, "peer-worker\n"), "thread comm '%s'", text);
  snprintf(path, sizeof(path), "/proc/%d/stat", tid);
  CHECK((text = slurp(path)) && atoi(text) == tid && strstr(text, " (peer-worker) "),
        "thread stat '%s'", text);
  snprintf(path, sizeof(path), "/proc/%d/status", child);
  CHECK((text = slurp(path)) && strstr(text, "Name:\tpeer-main\n") && strstr(text, "\nThreads:\t2\n") &&
            strstr(text, "\nUid:\t10057\t10057\t10057\t10057\n") &&
            strstr(text, "\nCapEff:\t0000000000000000\n"),
        "status '%s'", text);
  // The kernel's owner of /proc/<pid> is the process's effective ids.
  struct stat st;
  snprintf(path, sizeof(path), "/proc/%d", child);
  CHECK(stat(path, &st) == 0 && st.st_uid == 10057 && st.st_gid == 10057, "owner %d", st.st_uid);
  // Its thread's scheduling, set from here, reaches the thread.
  struct sched_param sp = {.sched_priority = 1};
  CHECK(sched_setscheduler(tid, SCHED_FIFO, &sp) == 0, "SCHED_FIFO");
  CHECK(setpriority(PRIO_PROCESS, tid, -10) == 0, "nice -10");
  CHECK(sched_getscheduler(tid) == SCHED_FIFO && getpriority(PRIO_PROCESS, tid) == -10,
        "read back");
  CHECK(sched_getscheduler(child) == SCHED_OTHER && getpriority(PRIO_PROCESS, child) == 0,
        "the main thread keeps its own");
  CHECK(sched_getscheduler(tid + 1) == -1 && errno == ESRCH, "no such thread");
  int ok = 0;
  write(peer_down[1], "c", 1);
  CHECK(read(peer_up[0], &ok, sizeof(ok)) == sizeof(ok) && ok, "the thread took it");
  // SIGCHLD and waitid carry the child's uid and pid.
  write(peer_down[1], "x", 1);
  siginfo_t si = {0};
  CHECK(waitid(P_PID, child, &si, WEXITED) == 0 && si.si_pid == child && si.si_uid == 10057,
        "waitid pid %d uid %d", si.si_pid, si.si_uid);
  for (int i = 0; i < 1000 && !chld_pid; i++) usleep(1000);
  CHECK(chld_pid == child && chld_uid == 10057, "SIGCHLD pid %d uid %d", chld_pid, chld_uid);
  signal(SIGCHLD, SIG_DFL);
  printf("ok peers\n");
}

// Without a process table, linux-run is the init of a private pid
// namespace: this prints its pid and a child's and exits, and the child
// dies with it (tests/process.rs).
static void ns_init_exit(void) {
  pid_t child = fork();
  CHECK(child >= 0, "fork");
  if (child == 0) {
    close(1);
    close(2);
    for (;;) pause();
  }
  printf("pids %d %d\n", getpid(), child);
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

// fork+exit+wait with a large address space: N mappings of alternating
// protection (so none merge), each with a dirty page, and M MiB touched in
// one more (argv: bench_mappings N M).
static void bench_mappings(int n, int mib) {
  enum { PAGE = 16384 };
  double s = now_us();
  for (int i = 0; i < n; i++) {
    char* p = mmap(NULL, 2 * PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    CHECK(p != MAP_FAILED, "mmap %d", i);
    p[0] = (char)i;
    if (i % 2) CHECK(mprotect(p + PAGE, PAGE, PROT_READ) == 0, "mprotect");
  }
  size_t big = (size_t)mib << 20;
  char* b = mmap(NULL, big, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  CHECK(b != MAP_FAILED, "big");
  for (size_t off = 0; off < big; off += PAGE) b[off] = 1;
  printf("bench setup %d mappings + %d MiB in %.0f ms\n", n, mib, (now_us() - s) / 1000);
  enum { R = 10 };
  double t[R];
  for (int i = 0; i < R; i++) {
    double s = now_us();
    pid_t pid = fork();
    if (pid == 0) _exit(b[big - PAGE] == 1 ? 0 : 1);
    int st;
    CHECK(waitpid(pid, &st, 0) == pid && WIFEXITED(st) && WEXITSTATUS(st) == 0, "child %#x", st);
    t[i] = now_us() - s;
  }
  qsort(t, R, sizeof(double), cmp);
  printf("bench %d mappings + %d MiB: fork+exit+wait p50 %.0f us p90 %.0f us\n", n, mib, t[R / 2],
         t[R * 9 / 10]);
  // How long the parent's fork call itself takes.
  for (int i = 0; i < R; i++) {
    double s = now_us();
    pid_t pid = fork();
    if (pid == 0) _exit(0);
    t[i] = now_us() - s;
    waitpid(pid, NULL, 0);
  }
  qsort(t, R, sizeof(double), cmp);
  printf("bench %d mappings + %d MiB: fork in the parent p50 %.0f us p90 %.0f us\n", n, mib,
         t[R / 2], t[R * 9 / 10]);
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
  main_argv = argv;
  main_args_end = argv[argc - 1] + strlen(argv[argc - 1]) + 1;
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
  if (strcmp(which, "pid_namespace") == 0 && argc == 3) {
    pid_namespace(atoi(argv[2]));
    return 0;
  }
  if (strcmp(which, "bench_mappings") == 0 && argc == 4) {
    bench_mappings(atoi(argv[2]), atoi(argv[3]));
    return 0;
  }
  struct {
    const char* name;
    void (*fn)(void);
  } checks[] = {
      {"fork_wait", fork_wait},     {"fork_new_code", fork_new_code},
      {"pipe_echo", pipe_echo},     {"mount_ns", mount_ns},
      {"exec_image", exec_image},   {"exec_argv", exec_argv},
      {"exec_script", exec_script}, {"waitid_variants", waitid_variants},
      {"pidfd_poll", pidfd_poll},   {"epoll_fork", epoll_fork},   {"death_by_signal", death_by_signal},
      {"fork_memory", fork_memory}, {"fork_then_exit", fork_then_exit},
      {"identity", identity},       {"identity_file", identity_file},
      {"seccomp_filter", seccomp_filter},
      {"xattrs", xattrs},           {"pf_key", pf_key},
      {"empty_rights", empty_rights}, {"own_files_thread", own_files_thread},
      {"permissions", permissions}, {"peers", peers},
      {"bench", bench},             {"ns_init_exit", ns_init_exit},
  };
  for (size_t i = 0; i < sizeof(checks) / sizeof(checks[0]); i++) {
    if (strcmp(which, checks[i].name) == 0 ||
        (strcmp(which, "all") == 0 && checks[i].fn != bench && checks[i].fn != identity_file &&
         checks[i].fn != ns_init_exit)) {
      fflush(stdout);
      checks[i].fn();
      fflush(stdout);
    }
  }
  return 0;
}
