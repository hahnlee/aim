// /proc and /sys as bionic, ART and system_server read them.
#include <dirent.h>
#include <dlfcn.h>
#include <fcntl.h>
#include <limits.h>
#include <pthread.h>
#include <sched.h>
#include <signal.h>
#include <stdint.h>
#include <stdlib.h>
#include <sys/prctl.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>

#include "check.h"

static char self_path[256];
static char** args;
static int nargs;

static ssize_t slurp(const char* path, char* buf, size_t cap) {
  int fd = open(path, O_RDONLY | O_CLOEXEC);
  if (fd < 0) return -1;
  size_t n = 0;
  ssize_t r;
  while (n + 1 < cap && (r = read(fd, buf + n, cap - 1 - n)) > 0) n += r;
  buf[n] = 0;
  close(fd);
  return n;
}

static char big[1 << 20];

static void maps_and_main_stack(void) {
  CHECK(slurp("/proc/self/maps", big, sizeof big) > 0);
  int local = 0;
  uintptr_t me = (uintptr_t)&local;
  int in_stack = 0, has_libc = 0, has_self = 0;
  for (char* line = strtok(big, "\n"); line; line = strtok(NULL, "\n")) {
    uintptr_t lo, hi;
    char perms[5];
    if (sscanf(line, "%lx-%lx %4s", &lo, &hi, perms) != 3) continue;
    if (strstr(line, "[stack]") && lo <= me && me < hi) in_stack = 1;
    if (strstr(line, "/apex/com.android.runtime/lib64/bionic/libc.so") && perms[2] == 'x') has_libc = 1;
    if (strstr(line, self_path)) has_self = 1;
  }
  CHECK(in_stack && has_libc && has_self);
  pthread_attr_t a;
  CHECK(pthread_getattr_np(pthread_self(), &a) == 0);
  void* base;
  size_t size;
  CHECK(pthread_attr_getstack(&a, &base, &size) == 0);
  CHECK((uintptr_t)base <= me && me < (uintptr_t)base + size);
}

static void status_stat_cmdline(void) {
  char want[64];
  CHECK(slurp("/proc/self/status", big, sizeof big) > 0);
  snprintf(want, sizeof want, "\nPid:\t%d\n", getpid());
  CHECK(strstr(big, want) && strstr(big, "\nThreads:\t") && strstr(big, "\nVmRSS:\t"));
  snprintf(want, sizeof want, "\nUid:\t%d\t", getuid());
  CHECK(strstr(big, want) != NULL);
  long rss_kb = strtol(strstr(big, "\nVmRSS:\t") + 8, NULL, 10);
  CHECK(slurp("/proc/self/stat", big, sizeof big) > 0);
  int pid = 0, ppid = 0;
  char state;
  CHECK(sscanf(big, "%d (%*[^)]) %c %d", &pid, &state, &ppid) == 3);
  CHECK(pid == getpid() && ppid == getppid());
  // rss (field 24) and statm count pages of the page size (proc(5)).
  char* f = strrchr(big, ')');
  for (int i = 2; i < 24; i++) f = strchr(f + 1, ' ');
  long page_kb = sysconf(_SC_PAGESIZE) / 1024, stat_kb = strtol(f + 1, NULL, 10) * page_kb;
  CHECK(slurp("/proc/self/statm", big, sizeof big) > 0);
  long size, resident;
  CHECK(sscanf(big, "%ld %ld", &size, &resident) == 2);
  CHECK(labs(stat_kb - rss_kb) < rss_kb / 4 && labs(resident * page_kb - rss_kb) < rss_kb / 4);
  ssize_t n = slurp("/proc/self/cmdline", big, sizeof big);
  CHECK(n > 0);
  char* q = big;
  for (int i = 0; i < nargs; i++) {
    CHECK(strcmp(q, args[i]) == 0);
    q += strlen(q) + 1;
  }
  CHECK(q == big + n);
  CHECK(slurp("/proc/self/comm", big, sizeof big) > 0 && strcmp(big, "t_proc\n") == 0);
  snprintf(want, sizeof want, "/proc/%d/status", getpid());
  CHECK(slurp(want, big, sizeof big) > 0);
}

static void fds_and_links(void) {
  char link[256];
  ssize_t n = readlink("/proc/self/exe", link, sizeof link - 1);
  CHECK(n > 0);
  link[n] = 0;
  CHECK(strcmp(link, self_path) == 0);
  int fd = open("/system/build.prop", O_RDONLY);
  char path[64];
  snprintf(path, sizeof path, "/proc/self/fd/%d", fd);
  n = readlink(path, link, sizeof link - 1);
  CHECK(n > 0);
  link[n] = 0;
  CHECK(strcmp(link, "/system/build.prop") == 0);
  struct stat a, b;
  CHECK(fstat(fd, &a) == 0 && stat(path, &b) == 0 && a.st_ino == b.st_ino);
  int again = open(path, O_RDONLY);
  CHECK(again >= 0 && fstat(again, &b) == 0 && a.st_ino == b.st_ino);
  close(again);
  int seen = 0;
  DIR* d = opendir("/proc/self/fd");
  CHECK(d != NULL);
  struct dirent* e;
  while ((e = readdir(d)) != NULL) {
    if (atoi(e->d_name) == fd) seen = 1;
  }
  closedir(d);
  CHECK(seen);
  int p[2];
  pipe(p);
  snprintf(path, sizeof path, "/proc/self/fd/%d", p[0]);
  n = readlink(path, link, sizeof link - 1);
  CHECK(n > 5 && strncmp(link, "pipe:[", 6) == 0);
  close(p[0]);
  close(p[1]);
  close(fd);
  CHECK(access("/proc/self/maps", R_OK) == 0);
  CHECK(access("/proc/self/nonexistent", F_OK) == -1 && errno == ENOENT);
  n = readlink("/proc/self", link, sizeof link - 1);
  CHECK(n > 0);
  link[n] = 0;
  CHECK(atoi(link) == getpid());
}

static void system_files(void) {
  CHECK(slurp("/proc/meminfo", big, sizeof big) > 0);
  long total = 0;
  CHECK(sscanf(big, "MemTotal: %ld kB", &total) == 1 && total > 1024 * 1024);
  CHECK(strstr(big, "MemAvailable:") != NULL);
  long ncpu = sysconf(_SC_NPROCESSORS_CONF);
  CHECK(ncpu >= 1 && sysconf(_SC_NPROCESSORS_ONLN) == ncpu);
  CHECK(slurp("/proc/cpuinfo", big, sizeof big) > 0);
  int procs = 0;
  for (char* s = big; (s = strstr(s, "processor\t:")); s++) procs++;
  CHECK(procs == ncpu && strstr(big, "Features\t: fp asimd") != NULL);
  char cpus[32];
  snprintf(cpus, sizeof cpus, "0-%ld\n", ncpu - 1);
  CHECK(slurp("/sys/devices/system/cpu/possible", big, sizeof big) > 0 && !strcmp(big, cpus));
  CHECK(slurp("/sys/devices/system/cpu/present", big, sizeof big) > 0 && !strcmp(big, cpus));
  CHECK(slurp("/proc/stat", big, sizeof big) > 0 && !strncmp(big, "cpu ", 4) && strstr(big, "\nbtime "));
  double up = 0, idle = 0;
  CHECK(slurp("/proc/uptime", big, sizeof big) > 0 && sscanf(big, "%lf %lf", &up, &idle) == 2 && up > 0);
  char id1[64], id2[64];
  CHECK(slurp("/proc/sys/kernel/random/boot_id", id1, sizeof id1) == 37 && id1[8] == '-');
  CHECK(slurp("/proc/sys/kernel/random/boot_id", id2, sizeof id2) == 37 && !strcmp(id1, id2));
  CHECK(slurp("/proc/sys/kernel/random/uuid", id2, sizeof id2) == 37 && strcmp(id1, id2));
  // A value init recorded under kernfs.
  CHECK(slurp("/proc/sys/kernel/panic_on_oops", big, sizeof big) > 0 && !strcmp(big, "1\n"));
  CHECK(slurp("/proc/version", big, sizeof big) > 0 && !strncmp(big, "Linux version", 13));
}

static void proc_dirs(void) {
  DIR* d = opendir("/proc");
  CHECK(d != NULL);
  int self = 0, mine = 0;
  struct dirent* e;
  while ((e = readdir(d)) != NULL) {
    if (!strcmp(e->d_name, "self")) self = 1;
    if (atoi(e->d_name) == getpid()) mine = e->d_type == DT_DIR;
  }
  closedir(d);
  CHECK(self && mine);
  struct stat st;
  CHECK(stat("/proc/self", &st) == 0 && S_ISDIR(st.st_mode));
  CHECK(lstat("/proc/self", &st) == 0 && S_ISLNK(st.st_mode));
  CHECK(stat("/proc/self/maps", &st) == 0 && S_ISREG(st.st_mode));
  int dfd = open("/proc/self", O_RDONLY | O_DIRECTORY);
  CHECK(dfd >= 0);
  int sfd = openat(dfd, "status", O_RDONLY);
  CHECK(sfd >= 0);
  close(sfd);
  close(dfd);
  d = opendir("/proc/self/task");
  CHECK(d != NULL);
  int tasks = 0;
  while ((e = readdir(d)) != NULL) {
    if (atoi(e->d_name) == getpid()) tasks++;
  }
  closedir(d);
  CHECK(tasks == 1);
  // bionic's realpath reads the link of an O_PATH fd (Rust's inherited-fd
  // scan of /proc/self/fd goes through it).
  char real[PATH_MAX], want[64];
  snprintf(want, sizeof want, "/proc/%d/fd", getpid());
  CHECK(realpath("/proc/self/fd", real) != NULL && !strcmp(real, want));
}

// vold, netd and libprocessgroup read the mount table.
static void mount_table(void) {
  CHECK(slurp("/proc/mounts", big, sizeof big) > 0);
  CHECK(!strncmp(big, "/dev/root / erofs ro,", 21));
  CHECK(strstr(big, "\nproc /proc proc rw,") && strstr(big, " /data ext4 rw,") &&
        strstr(big, "\ntmpfs /dev tmpfs rw,"));
  CHECK(slurp("/proc/self/mountinfo", big, sizeof big) > 0 && !strncmp(big, "1 0 0:1 / / ro,", 15));
  CHECK(strstr(big, " / /proc rw,nosuid,nodev,noexec,relatime - proc proc rw\n") != NULL);
}

// A second thread for the per-thread files: it names itself, reports its
// tid and waits until told to finish.
static pid_t worker_tid;
static volatile int worker_hits;
static int worker_go[2];

static void on_usr1(int sig) {
  (void)sig;
  if (gettid() == worker_tid) worker_hits++;
}

static void* worker(void* arg) {
  (void)arg;
  worker_tid = gettid();
  char c;
  read(worker_go[0], &c, 1);
  return NULL;
}

// Thread ids are 2^17 + (pid << 11) + n, above every pid and below 2^28: /proc, the scheduler calls and
// tgkill accept them, and a thread's comm follows pthread_setname_np.
static void threads_and_names(void) {
  CHECK(pipe(worker_go) == 0);
  signal(SIGUSR1, on_usr1);
  pthread_t t;
  CHECK(pthread_create(&t, NULL, worker, NULL) == 0);
  while (!__atomic_load_n(&worker_tid, __ATOMIC_SEQ_CST)) usleep(1000);
  CHECK(worker_tid > (1 << 17) && worker_tid < (1 << 28));
  char path[96], name[32];
  snprintf(path, sizeof path, "/proc/self/task/%d", worker_tid);
  struct stat st;
  CHECK(stat(path, &st) == 0 && S_ISDIR(st.st_mode));
  snprintf(path, sizeof path, "/proc/%d/status", worker_tid);
  CHECK(slurp(path, big, sizeof big) > 0);
  int listed = 0;
  DIR* d = opendir("/proc/self/task");
  CHECK(d != NULL);
  struct dirent* e;
  while ((e = readdir(d)) != NULL) {
    if (atoi(e->d_name) == worker_tid) listed = 1;
  }
  closedir(d);
  CHECK(listed);
  // A value written for a thread (task profiles' timer slack) is kept, and
  // the thread stays out of /proc's list of processes.
  snprintf(path, sizeof path, "/proc/%d/timerslack_ns", worker_tid);
  int slack = open(path, O_WRONLY | O_CLOEXEC);
  CHECK(slack >= 0 && write(slack, "50000", 5) == 5);
  close(slack);
  CHECK(stat(path, &st) == 0);
  listed = 0;
  d = opendir("/proc");
  CHECK(d != NULL);
  while ((e = readdir(d)) != NULL) {
    if (atoi(e->d_name) == worker_tid) listed = 1;
  }
  closedir(d);
  CHECK(!listed);
  // Another thread's name goes through /proc/self/task/<tid>/comm.
  CHECK(pthread_setname_np(t, "worker-x") == 0);
  snprintf(path, sizeof path, "/proc/self/task/%d/comm", worker_tid);
  CHECK(slurp(path, big, sizeof big) > 0 && !strcmp(big, "worker-x\n"));
  CHECK(pthread_getname_np(t, name, sizeof name) == 0 && !strcmp(name, "worker-x"));
  // The calling thread's own name (prctl) is the process's comm here.
  CHECK(prctl(PR_SET_NAME, "main-y") == 0);
  CHECK(slurp("/proc/self/comm", big, sizeof big) > 0 && !strcmp(big, "main-y\n"));
  snprintf(path, sizeof path, "/proc/self/task/%d/comm", getpid());
  CHECK(slurp(path, big, sizeof big) > 0 && !strcmp(big, "main-y\n"));
  struct sched_param sp = {.sched_priority = 0};
  CHECK(sched_setscheduler(worker_tid, SCHED_BATCH, &sp) == 0);
  CHECK(sched_getscheduler(worker_tid) == SCHED_BATCH);
  CHECK(setpriority(PRIO_PROCESS, worker_tid, 5) == 0);
  errno = 0;
  CHECK(getpriority(PRIO_PROCESS, worker_tid) == 5 && errno == 0);
  CHECK(syscall(SYS_tgkill, getpid(), worker_tid, SIGUSR1) == 0);
  for (int i = 0; i < 1000 && !worker_hits; i++) usleep(1000);
  CHECK(worker_hits == 1);
  CHECK(write(worker_go[1], "x", 1) == 1);
  CHECK(pthread_join(t, NULL) == 0);
  CHECK(syscall(SYS_tgkill, getpid(), worker_tid, 0) == -1 && errno == ESRCH);
  // The value goes with the thread.
  snprintf(path, sizeof path, "/proc/%d/timerslack_ns", worker_tid);
  CHECK(stat(path, &st) == -1 && errno == ENOENT);
  prctl(PR_SET_NAME, "t_proc");
}

// libselinux's view: every class of the image's policy is known and every
// check is allowed, and ftrace's marker takes writes.
static void selinuxfs_and_tracing(void) {
  char buf[64];
  CHECK(slurp("/sys/fs/selinux/class/service_manager/index", buf, sizeof buf) > 0 && atoi(buf) > 0);
  CHECK(slurp("/sys/fs/selinux/class/service_manager/perms/find", buf, sizeof buf) > 0 &&
        atoi(buf) == 2);
  int perms = 0;
  DIR* d = opendir("/sys/fs/selinux/class/file/perms");
  CHECK(d != NULL);
  struct dirent* e;
  while ((e = readdir(d)) != NULL) perms += e->d_name[0] != '.';
  closedir(d);
  CHECK(perms > 20);
  int fd = open("/sys/fs/selinux/access", O_RDWR | O_CLOEXEC);
  CHECK(fd >= 0);
  const char* q = "u:r:servicemanager:s0 u:object_r:service_manager:s0 2 2";
  CHECK(write(fd, q, strlen(q)) == (ssize_t)strlen(q));
  memset(buf, 0, sizeof buf);
  CHECK(read(fd, buf, sizeof buf - 1) > 0);
  unsigned allowed = 0, decided = 0;
  CHECK(sscanf(buf, "%x %x", &allowed, &decided) == 2 && allowed == ~0u && decided == ~0u);
  close(fd);
  void* se = dlopen("libselinux.so", RTLD_NOW);
  if (se) {
    int (*check)(const char*, const char*, const char*, const char*, void*) =
        dlsym(se, "selinux_check_access");
    CHECK(check && check("u:r:servicemanager:s0", "u:object_r:service_manager:s0",
                         "service_manager", "find", NULL) == 0);
  } else {
    printf("note: libselinux.so not loadable here: %s\n", dlerror());
  }
  fd = open("/sys/kernel/tracing/trace_marker", O_WRONLY | O_CLOEXEC);
  CHECK(fd >= 0 && write(fd, "B|1|x", 5) == 5);
  close(fd);
  fd = open("/sys/kernel/debug/tracing/trace_marker", O_WRONLY | O_CLOEXEC);
  CHECK(fd >= 0 && write(fd, "E|1", 3) == 3);
  close(fd);
  CHECK(slurp("/sys/kernel/tracing/tracing_on", buf, sizeof buf) > 0 && !strcmp(buf, "0\n"));
}

int main(int argc, char** argv) {
  args = argv;
  nargs = argc;
  snprintf(self_path, sizeof self_path, "%s", argv[0]);
  RUN(maps_and_main_stack);
  RUN(status_stat_cmdline);
  RUN(fds_and_links);
  RUN(system_files);
  RUN(proc_dirs);
  RUN(mount_table);
  RUN(threads_and_names);
  RUN(selinuxfs_and_tracing);
  DONE();
}
