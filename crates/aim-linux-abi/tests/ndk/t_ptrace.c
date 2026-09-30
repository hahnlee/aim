// ptrace, process_vm_readv/writev, /proc of another process and signals
// to it, as debuggerd and crash_dump use them (ptrace(2),
// process_vm_readv(2), proc(5), sigqueue(3); after LTP's ptrace,
// process_vm and kill tests).
#include <dirent.h>
#include <elf.h>
#include <fcntl.h>
#include <linux/futex.h>
#include <pthread.h>
#include <sched.h>
#include <signal.h>
#include <stdint.h>
#include <stdlib.h>
#include <sys/ptrace.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/uio.h>
#include <sys/user.h>
#include <sys/wait.h>
#include <unistd.h>

#include "check.h"

#define TRAP_STOP (SIGTRAP | PTRACE_EVENT_STOP << 8)

static volatile uint64_t magic;
static volatile uint64_t spins;
static char buffer[64];

// A child that blocks in read(2) on `in` until the parent writes a byte;
// it exits with 7 when that byte is 'x'.
static int blocked_fd;

static pid_t blocked_child(int* in_w) {
  int p[2];
  if (pipe(p) != 0) return -1;
  blocked_fd = p[0];
  pid_t c = fork();
  if (c == 0) {
    close(p[1]);
    magic = 0x1234567890abcdefULL;
    char b = 0;
    _exit(read(p[0], &b, 1) == 1 && b == 'x' ? 7 : 1);
  }
  close(p[0]);
  *in_w = p[1];
  return c;
}

static int stop_status(pid_t tid) {
  int st = 0;
  return waitpid(tid, &st, __WALL) == tid ? st : -1;
}

static int regs_of(pid_t tid, struct user_regs_struct* r) {
  struct iovec iov = {r, sizeof *r};
  return ptrace(PTRACE_GETREGSET, tid, NT_PRSTATUS, &iov) == 0 && iov.iov_len == sizeof *r ? 0
                                                                                             : -1;
}

// PTRACE_INTERRUPT stops a thread blocked in a syscall with PTRACE_EVENT_STOP;
// its registers show the syscall about to restart (x8 its number, x0 its
// first argument again); PEEKDATA and NT_ARM_TLS read the tracee; CONT
// resumes it and the syscall completes.
static void interrupt_a_blocked_syscall(void) {
  int w;
  pid_t c = blocked_child(&w);
  CHECK(c > 0);
  CHECK(ptrace(PTRACE_SEIZE, c, 0, 0) == 0);
  struct user_regs_struct r;
  int tries = 0;
  for (;;) {
    CHECK(ptrace(PTRACE_INTERRUPT, c, 0, 0) == 0);
    int st = stop_status(c);
    CHECK(WIFSTOPPED(st) && st >> 8 == TRAP_STOP);
    CHECK(regs_of(c, &r) == 0);
    if (r.regs[8] == __NR_read && r.regs[0] == (uint64_t)blocked_fd) break;
    // Not in read(2) yet.
    CHECK(++tries < 200);
    CHECK(ptrace(PTRACE_CONT, c, 0, 0) == 0);
    usleep(10000);
  }
  errno = 0;
  CHECK(ptrace(PTRACE_PEEKDATA, c, (void*)&magic, 0) == (long)0x1234567890abcdefULL && errno == 0);
  uint64_t tls[2] = {0, 0};
  struct iovec iov = {tls, sizeof tls};
  CHECK(ptrace(PTRACE_GETREGSET, c, NT_ARM_TLS, &iov) == 0 && iov.iov_len == sizeof tls);
  uint64_t mine;
  __asm__ volatile("mrs %0, tpidr_el0" : "=r"(mine));
  CHECK(tls[0] == mine);  // A fork keeps the forking thread's pointer.
  struct user_fpsimd_struct fp;
  iov = (struct iovec){&fp, sizeof fp};
  CHECK(ptrace(PTRACE_GETREGSET, c, NT_PRFPREG, &iov) == 0 && iov.iov_len == sizeof fp);
  iov = (struct iovec){&fp, 6};
  CHECK(ptrace(PTRACE_GETREGSET, c, NT_PRSTATUS, &iov) == -1 && errno == EINVAL);
  iov = (struct iovec){&fp, sizeof fp};
  CHECK(ptrace(PTRACE_GETREGSET, c, 0x999, &iov) == -1 && errno == EINVAL);
  errno = 0;
  CHECK(ptrace(PTRACE_PEEKDATA, c, (void*)8, 0) == -1 && errno == EIO);
  CHECK(ptrace(PTRACE_CONT, c, 0, 99) == -1 && errno == EIO);
  CHECK(ptrace(PTRACE_CONT, c, 0, 0) == 0);
  CHECK(ptrace(PTRACE_CONT, c, 0, 0) == -1 && errno == ESRCH);  // running
  CHECK(write(w, "x", 1) == 1);
  int st = 0;
  CHECK(waitpid(c, &st, 0) == c && WIFEXITED(st) && WEXITSTATUS(st) == 7);
  close(w);
}

__attribute__((noinline)) static void spin(void) {
  for (;;) spins++;
}

// A thread running guest code stops where it is; DETACH lets it run on.
static void interrupt_guest_code(void) {
  pid_t c = fork();
  if (c == 0) spin();
  CHECK(c > 0);
  CHECK(ptrace(PTRACE_SEIZE, c, 0, 0) == 0);
  struct user_regs_struct r;
  for (int tries = 0;; tries++) {
    CHECK(ptrace(PTRACE_INTERRUPT, c, 0, 0) == 0);
    CHECK(stop_status(c) >> 8 == TRAP_STOP);
    CHECK(regs_of(c, &r) == 0);
    if (r.pc >= (uintptr_t)spin && r.pc < (uintptr_t)spin + 64) break;
    // Not spinning yet.
    CHECK(tries < 200);
    CHECK(ptrace(PTRACE_CONT, c, 0, 0) == 0);
    usleep(10000);
  }
  char path[64], buf[4096];
  snprintf(path, sizeof path, "/proc/%d/status", c);
  int fd = open(path, O_RDONLY);
  CHECK(fd >= 0);
  ssize_t n = read(fd, buf, sizeof buf - 1);
  close(fd);
  CHECK(n > 0);
  buf[n] = 0;
  char want[32];
  snprintf(want, sizeof want, "TracerPid:\t%d\n", getpid());
  CHECK(strstr(buf, want));
  CHECK(ptrace(PTRACE_DETACH, c, 0, 0) == 0);
  CHECK(ptrace(PTRACE_INTERRUPT, c, 0, 0) == -1 && errno == ESRCH);
  kill(c, SIGKILL);
  int st = 0;
  CHECK(waitpid(c, &st, 0) == c && WIFSIGNALED(st) && WTERMSIG(st) == SIGKILL);
}

// Requests that fail as on Linux.
static void errors(void) {
  CHECK(ptrace(PTRACE_SEIZE, getpid(), 0, 0) == -1 && errno == EPERM);
  CHECK(ptrace(PTRACE_INTERRUPT, getpid(), 0, 0) == -1 && errno == ESRCH);
  CHECK(ptrace(PTRACE_INTERRUPT, 99999, 0, 0) == -1 && errno == ESRCH);
  int w;
  pid_t c = blocked_child(&w);
  CHECK(c > 0);
  CHECK(ptrace(PTRACE_INTERRUPT, c, 0, 0) == -1 && errno == ESRCH);  // not traced
  CHECK(ptrace(PTRACE_SEIZE, c, 1, 0) == -1 && errno == EIO);
  CHECK(ptrace(PTRACE_SEIZE, c, 0, 0x80000000) == -1 && errno == EINVAL);
  CHECK(ptrace(PTRACE_SEIZE, c, 0, 0) == 0);
  CHECK(ptrace(PTRACE_SEIZE, c, 0, 0) == -1 && errno == EPERM);  // traced already
  unsigned long msg;
  CHECK(ptrace(PTRACE_GETEVENTMSG, c, 0, &msg) == -1 && errno == ESRCH);  // running
  CHECK(ptrace(PTRACE_INTERRUPT, c, 0, 0) == 0);
  CHECK(stop_status(c) >> 8 == TRAP_STOP);
  CHECK(ptrace(0x4299, c, 0, 0) == -1 && errno == EIO);
  CHECK(ptrace(PTRACE_SETOPTIONS, c, 0, 0x80000000) == -1 && errno == EINVAL);
  CHECK(ptrace(PTRACE_DETACH, c, 0, 0) == 0);
  CHECK(write(w, "x", 1) == 1);
  int st = 0;
  CHECK(waitpid(c, &st, 0) == c && WIFEXITED(st) && WEXITSTATUS(st) == 7);
  close(w);
}

// process_vm_readv/writev on another process: whole, partial and failing
// transfers.
static void other_process_memory(void) {
  int w;
  strcpy(buffer, "child's buffer");
  pid_t c = blocked_child(&w);
  CHECK(c > 0);
  strcpy(buffer, "changed here");
  char got[64] = {0};
  struct iovec l = {got, sizeof got}, r = {buffer, 15};
  CHECK(process_vm_readv(c, &l, 1, &r, 1, 0) == 15 && strcmp(got, "child's buffer") == 0);
  struct iovec rs[2] = {{buffer, 8}, {(void*)16, 8}};
  CHECK(process_vm_readv(c, &l, 1, rs, 2, 0) == 8);
  r = (struct iovec){(void*)16, 8};
  CHECK(process_vm_readv(c, &l, 1, &r, 1, 0) == -1 && errno == EFAULT);
  struct iovec src = {"written", 8};
  r = (struct iovec){buffer, 8};
  CHECK(process_vm_writev(c, &src, 1, &r, 1, 0) == 8);
  memset(got, 0, sizeof got);
  CHECK(process_vm_readv(c, &l, 1, &r, 1, 0) == 8 && strcmp(got, "written") == 0);
  CHECK(process_vm_readv(99999, &l, 1, &r, 1, 0) == -1 && errno == ESRCH);
  CHECK(write(w, "x", 1) == 1);
  int st = 0;
  CHECK(waitpid(c, &st, 0) == c && WIFEXITED(st));
  close(w);
}

// /proc/<pid>/maps and /proc/<pid>/fd of another process.
static void other_process_proc(void) {
  int w;
  pid_t c = blocked_child(&w);
  CHECK(c > 0);
  char path[64], buf[65536];
  snprintf(path, sizeof path, "/proc/%d/maps", c);
  int fd = open(path, O_RDONLY);
  CHECK(fd >= 0);
  size_t n = 0;
  ssize_t k;
  while (n < sizeof buf - 1 && (k = read(fd, buf + n, sizeof buf - 1 - n)) > 0) n += k;
  close(fd);
  buf[n] = 0;
  CHECK(strstr(buf, "/data/local/tmp/t_ptrace\n") && strstr(buf, "[stack]"));
  snprintf(path, sizeof path, "/proc/%d/fd", c);
  DIR* d = opendir(path);
  CHECK(d);
  int pipes = 0;
  struct dirent* e;
  while ((e = readdir(d))) {
    if (e->d_name[0] == '.') continue;
    char link[128], target[128];
    snprintf(link, sizeof link, "%s/%s", path, e->d_name);
    ssize_t m = readlink(link, target, sizeof target - 1);
    if (m > 0) {
      target[m] = 0;
      pipes += strncmp(target, "pipe:[", 6) == 0;
    }
  }
  closedir(d);
  CHECK(pipes >= 1);
  snprintf(path, sizeof path, "/proc/%d/fd/9999", c);
  char t[16];
  CHECK(readlink(path, t, sizeof t) == -1 && errno == ENOENT);
  CHECK(write(w, "x", 1) == 1);
  int st = 0;
  CHECK(waitpid(c, &st, 0) == c && WIFEXITED(st));
  close(w);
}

static void* short_thread(void* arg) {
  (void)arg;
  return NULL;
}

// PTRACE_O_TRACECLONE: a traced thread's clone(CLONE_FILES) child and its
// new thread start traced, stopped, and the tracer sees the event, the new
// tid and their ends.
static void trace_clone_events(void) {
  int go[2];
  CHECK(pipe(go) == 0);
  pid_t c = fork();
  if (c == 0) {
    char b;
    if (read(go[0], &b, 1) != 1) _exit(1);
    pid_t g = syscall(__NR_clone, CLONE_FILES, 0, 0, 0, 0);
    if (g == 0) _exit(3);
    int st = 0;
    if (g < 0 || waitpid(g, &st, __WCLONE) != g || !WIFEXITED(st) || WEXITSTATUS(st) != 3)
      _exit(2);
    pthread_t t;
    if (pthread_create(&t, NULL, short_thread, NULL) != 0 || pthread_join(t, NULL) != 0) _exit(4);
    _exit(0);
  }
  CHECK(c > 0);
  CHECK(ptrace(PTRACE_SEIZE, c, 0, PTRACE_O_TRACECLONE) == 0);
  CHECK(write(go[1], "g", 1) == 1);
  CHECK(stop_status(c) >> 8 == (SIGTRAP | PTRACE_EVENT_CLONE << 8));
  unsigned long g = 0;
  CHECK(ptrace(PTRACE_GETEVENTMSG, c, 0, &g) == 0 && g > 0);
  CHECK(stop_status(g) >> 8 == TRAP_STOP);
  CHECK(ptrace(PTRACE_CONT, g, 0, 0) == 0);
  int st = stop_status(g);
  CHECK(WIFEXITED(st) && WEXITSTATUS(st) == 3);
  CHECK(ptrace(PTRACE_CONT, c, 0, 0) == 0);
  CHECK(stop_status(c) >> 8 == (SIGTRAP | PTRACE_EVENT_CLONE << 8));
  unsigned long tid = 0;
  CHECK(ptrace(PTRACE_GETEVENTMSG, c, 0, &tid) == 0 && tid > 0);
  CHECK(stop_status(tid) >> 8 == TRAP_STOP);
  CHECK(ptrace(PTRACE_CONT, tid, 0, 0) == 0);
  CHECK(ptrace(PTRACE_CONT, c, 0, 0) == 0);
  st = stop_status(tid);
  CHECK(WIFEXITED(st) && WEXITSTATUS(st) == 0);
  CHECK(waitpid(c, &st, 0) == c && WIFEXITED(st) && WEXITSTATUS(st) == 0);
  close(go[0]);
  close(go[1]);
}

struct pseudo {
  pid_t main;
  int result;
  int go;
};

static int pseudothread(void* arg) {
  struct pseudo* p = arg;
  pid_t c = syscall(__NR_clone, SIGCHLD, 0, 0, 0, 0);
  if (c == 0) {
    char ok = syscall(__NR_getppid) == p->main ? 'y' : 'n';
    write(p->result, &ok, 1);
    _exit(0);
  }
  int st;
  waitpid(c, &st, 0);
  char b;
  read(p->go, &b, 1);
  return 0;
}

// A thread with its own file table (debuggerd's pseudothread): a task of
// its process, and its children's parent is that process.
static void own_files_thread(void) {
  int res[2], go[2];
  CHECK(pipe(res) == 0 && pipe(go) == 0);
  struct pseudo p = {getpid(), res[1], go[0]};
  static char stack[65536] __attribute__((aligned(16)));
  static pid_t tid_word;
  pid_t t = clone(pseudothread, stack + sizeof stack,
                  CLONE_THREAD | CLONE_SIGHAND | CLONE_VM | CLONE_CHILD_SETTID |
                      CLONE_CHILD_CLEARTID,
                  &p, NULL, NULL, &tid_word);
  CHECK(t > 0);
  char ok = 0;
  CHECK(read(res[0], &ok, 1) == 1 && ok == 'y');
  char path[64];
  struct stat s;
  snprintf(path, sizeof path, "/proc/%d/task/%d", getpid(), t);
  CHECK(stat(path, &s) == 0 && S_ISDIR(s.st_mode));
  CHECK(write(go[1], "g", 1) == 1);
  for (int i = 0; i < 500 && __atomic_load_n(&tid_word, __ATOMIC_SEQ_CST) != 0; i++) usleep(10000);
  CHECK(tid_word == 0);
  close(res[0]);
  close(res[1]);
  close(go[0]);
  close(go[1]);
}

static int sig_out;

static void record(int sig, siginfo_t* si, void* uc) {
  (void)uc;
  int r[4] = {sig, si->si_code, si->si_pid, si->si_value.sival_int};
  write(sig_out, r, sizeof r);
}

// kill(SIGSEGV), sigqueue() of a real-time signal and tgkill() to another
// process arrive with the sender's pid and code, not as faults.
static void signals_to_another_process(void) {
  int p[2], ready[2];
  CHECK(pipe(p) == 0 && pipe(ready) == 0);
  pid_t c = fork();
  if (c == 0) {
    sig_out = p[1];
    struct sigaction sa = {.sa_sigaction = record, .sa_flags = SA_SIGINFO};
    sigaction(SIGSEGV, &sa, NULL);
    sigaction(SIGRTMAX - 1, &sa, NULL);
    sigaction(SIGUSR1, &sa, NULL);
    write(ready[1], "r", 1);
    for (;;) pause();
  }
  CHECK(c > 0);
  char b;
  CHECK(read(ready[0], &b, 1) == 1);
  int r[4];
  CHECK(kill(c, SIGSEGV) == 0);
  CHECK(read(p[0], r, sizeof r) == sizeof r);
  CHECK(r[0] == SIGSEGV && r[1] == SI_USER && r[2] == getpid());
  CHECK(sigqueue(c, SIGRTMAX - 1, (union sigval){.sival_int = 42}) == 0);
  CHECK(read(p[0], r, sizeof r) == sizeof r);
  CHECK(r[0] == SIGRTMAX - 1 && r[1] == SI_QUEUE && r[2] == getpid() && r[3] == 42);
  CHECK(syscall(__NR_tgkill, c, c, SIGUSR1) == 0);
  CHECK(read(p[0], r, sizeof r) == sizeof r);
  CHECK(r[0] == SIGUSR1 && r[1] == SI_TKILL && r[2] == getpid());
  siginfo_t si = {.si_code = SI_USER};
  CHECK(syscall(__NR_rt_sigqueueinfo, c, SIGUSR1, &si) == -1 && errno == EPERM);
  kill(c, SIGKILL);
  int st;
  CHECK(waitpid(c, &st, 0) == c && WIFSIGNALED(st));
  close(p[0]);
  close(p[1]);
  close(ready[0]);
  close(ready[1]);
}

int main(void) {
  RUN(interrupt_a_blocked_syscall);
  RUN(interrupt_guest_code);
  RUN(errors);
  RUN(other_process_memory);
  RUN(other_process_proc);
  RUN(trace_clone_events);
  RUN(own_files_thread);
  RUN(signals_to_another_process);
  DONE();
}
