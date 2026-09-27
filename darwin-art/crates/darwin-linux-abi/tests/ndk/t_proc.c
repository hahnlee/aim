// /proc and /sys as bionic, ART and system_server read them.
#include <dirent.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdint.h>
#include <stdlib.h>
#include <sys/stat.h>
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
  CHECK(slurp("/proc/self/stat", big, sizeof big) > 0);
  int pid = 0, ppid = 0;
  char state;
  CHECK(sscanf(big, "%d (%*[^)]) %c %d", &pid, &state, &ppid) == 3);
  CHECK(pid == getpid() && ppid == getppid());
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
  DONE();
}
