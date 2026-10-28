// cgroup v1 hierarchies as libprocessgroup uses them: the mounts it finds in
// /proc/mounts, groups made and removed, tasks moved with tasks and
// cgroup.procs (alone and with their thread group), what /proc/<pid>/cgroup
// shows, inheritance by new threads and fork children, the control
// files' values and errors, and that the files keeping the membership are
// none of the process's fds.
#include <dirent.h>
#include <fcntl.h>
#include <pthread.h>
#include <sched.h>
#include <linux/futex.h>
#include <stdlib.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/statfs.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <unistd.h>

#include "check.h"

static int put(const char* path, const char* value) {
  int fd = open(path, O_WRONLY | O_CLOEXEC);
  if (fd < 0) return -1;
  ssize_t n = write(fd, value, strlen(value));
  int saved = errno;
  close(fd);
  errno = saved;
  return n == (ssize_t)strlen(value) ? 0 : -1;
}

static int get(const char* path, char* out, size_t size) {
  int fd = open(path, O_RDONLY | O_CLOEXEC);
  if (fd < 0) return -1;
  ssize_t n = read(fd, out, size - 1);
  close(fd);
  if (n < 0) return -1;
  out[n] = 0;
  return 0;
}

// The line of /proc/<task>/cgroup for `controller`, without its newline.
static int group_of(const char* task, const char* controller, char* out, size_t size) {
  char path[64], all[512], tag[32];
  snprintf(path, sizeof path, "/proc/%s/cgroup", task);
  if (get(path, all, sizeof all) != 0) return -1;
  snprintf(tag, sizeof tag, ":%s:", controller);
  char* at = strstr(all, tag);
  if (!at) return -1;
  at += strlen(tag);
  size_t n = strcspn(at, "\n");
  if (n >= size) return -1;
  memcpy(out, at, n);
  out[n] = 0;
  return 0;
}

static int lists(const char* path, long id) {
  char text[4096], want[32];
  if (get(path, text, sizeof text) != 0) return 0;
  snprintf(want, sizeof want, "%ld\n", id);
  for (char* line = text; *line; line = strchr(line, '\n') + 1) {
    if (strncmp(line, want, strlen(want)) == 0) return 1;
    if (!strchr(line, '\n')) break;
  }
  return 0;
}

static void mounts(void) {
  char text[8192];
  CHECK(get("/proc/mounts", text, sizeof text) == 0);
  CHECK(strstr(text, "none /dev/cpuset cgroup rw,nosuid,nodev,noexec,relatime,cpuset,noprefix,"
                     "release_agent=/sbin/cpuset_release_agent 0 0\n"));
  CHECK(strstr(text, "none /dev/cpuctl cgroup rw,nosuid,nodev,noexec,relatime,cpu 0 0\n"));
  struct statfs s;
  CHECK(statfs("/dev/cpuset", &s) == 0 && s.f_type == 0x27e0eb);
  char line[64];
  CHECK(group_of("self", "cpuset", line, sizeof line) == 0 && strcmp(line, "/") == 0);
  CHECK(get("/proc/self/cgroup", text, sizeof text) == 0 && strstr(text, "0::/\n"));
  // The root lists every task, this one among them.
  CHECK(lists("/dev/cpuset/tasks", getpid()));
}

static void* report(void* arg) {
  char* out = arg;
  char task[32];
  snprintf(task, sizeof task, "self/task/%ld", (long)syscall(SYS_gettid));
  if (group_of(task, "cpuset", out, 64) != 0) strcpy(out, "?");
  return NULL;
}

static void moves(void) {
  CHECK(mkdir("/dev/cpuset/bg", 0755) == 0);
  struct stat st;
  CHECK(stat("/dev/cpuset/bg/tasks", &st) == 0 && (st.st_mode & 0777) == 0644);
  char value[64];
  CHECK(get("/dev/cpuset/bg/cpus", value, sizeof value) == 0 && strcmp(value, "\n") == 0);
  char pid[16];
  snprintf(pid, sizeof pid, "%d", getpid());
  // A cpuset without cpus takes no task.
  CHECK(put("/dev/cpuset/bg/cgroup.procs", pid) < 0 && errno == ENOSPC);
  CHECK(get("/dev/cpuset/cpus", value, sizeof value) == 0);
  CHECK(put("/dev/cpuset/bg/cpus", value) == 0 && put("/dev/cpuset/bg/mems", "0") == 0);
  CHECK(put("/dev/cpuset/bg/cgroup.procs", "abc") < 0 && errno == EINVAL);
  CHECK(put("/dev/cpuset/bg/cgroup.procs", "99999999") < 0 && errno == ESRCH);
  CHECK(put("/dev/cpuset/bg/cgroup.procs", pid) == 0);
  CHECK(group_of("self", "cpuset", value, sizeof value) == 0 && strcmp(value, "/bg") == 0);
  CHECK(lists("/dev/cpuset/bg/cgroup.procs", getpid()) && lists("/dev/cpuset/bg/tasks", getpid()));
  CHECK(!lists("/dev/cpuset/tasks", getpid()));
  // A cpuset with tasks keeps its cpus.
  CHECK(put("/dev/cpuset/bg/cpus", "") < 0 && errno == ENOSPC);
  // A new thread is in its creator's group.
  pthread_t t;
  char seen[64];
  CHECK(pthread_create(&t, NULL, report, seen) == 0 && pthread_join(t, NULL) == 0);
  CHECK(strcmp(seen, "/bg") == 0);
  // A fork child is in its forking thread's.
  FORK_OR_SKIP(child);
  if (child == 0) {
    char g[64];
    _exit(group_of("self", "cpuset", g, sizeof g) == 0 && strcmp(g, "/bg") == 0 ? 0 : 1);
  }
  int status;
  CHECK(waitpid(child, &status, 0) == child && WIFEXITED(status) && WEXITSTATUS(status) == 0);
  // A group with tasks or children cannot go.
  CHECK(rmdir("/dev/cpuset/bg") < 0 && errno == EBUSY);
  // tasks moves one thread: this one to the root, the process stays.
  char tid[16];
  snprintf(tid, sizeof tid, "%ld", (long)syscall(SYS_gettid));
  CHECK(put("/dev/cpuset/tasks", tid) == 0);
  snprintf(seen, sizeof seen, "self/task/%s", tid);
  CHECK(group_of(seen, "cpuset", value, sizeof value) == 0 && strcmp(value, "/") == 0);
  CHECK(put("/dev/cpuset/cgroup.procs", pid) == 0);
  CHECK(group_of("self", "cpuset", value, sizeof value) == 0 && strcmp(value, "/") == 0);
  // Control files cannot be made or removed; empty groups can go.
  CHECK(open("/dev/cpuset/bg/extra", O_WRONLY | O_CREAT, 0644) < 0 && errno == EACCES);
  CHECK(unlink("/dev/cpuset/bg/cpus") < 0 && errno == EPERM);
  CHECK(rmdir("/dev/cpuset/bg") == 0);
  CHECK(access("/dev/cpuset/bg", F_OK) < 0 && errno == ENOENT);
}

static void values(void) {
  char value[64];
  CHECK(mkdir("/dev/cpuctl/g", 0755) == 0);
  // The root group's weight is fixed; a child's is clamped.
  CHECK(put("/dev/cpuctl/cpu.shares", "512") < 0 && errno == EINVAL);
  CHECK(put("/dev/cpuctl/g/cpu.shares", "1") == 0);
  CHECK(get("/dev/cpuctl/g/cpu.shares", value, sizeof value) == 0 && strcmp(value, "2\n") == 0);
  CHECK(get("/dev/cpuctl/g/cpu.uclamp.max", value, sizeof value) == 0 && strcmp(value, "max\n") == 0);
  CHECK(put("/dev/cpuctl/g/cpu.uclamp.min", "12.345") == 0);
  CHECK(get("/dev/cpuctl/g/cpu.uclamp.min", value, sizeof value) == 0 &&
        strcmp(value, "12.35\n") == 0);
  CHECK(put("/dev/cpuctl/g/cpu.uclamp.min", "101") < 0 && errno == ERANGE);
  CHECK(access("/dev/cpuctl/cpu.uclamp.min", F_OK) < 0 && errno == ENOENT);
  // O_TRUNC leaves a control file; each write is a value of its own.
  int fd = open("/dev/cpuctl/g/cpu.idle", O_RDWR | O_TRUNC);
  CHECK(fd >= 0);
  CHECK(write(fd, "1", 1) == 1 && write(fd, "2", 1) < 0 && errno == EINVAL);
  CHECK(lseek(fd, 0, SEEK_SET) == 0 && read(fd, value, sizeof value) == 2 && value[0] == '1');
  CHECK(lseek(fd, 0, SEEK_END) < 0 && errno == EINVAL);
  close(fd);
  // A group renames within its parent, its tasks with it.
  char pid[16];
  snprintf(pid, sizeof pid, "%d", getpid());
  CHECK(put("/dev/cpuctl/g/cgroup.procs", pid) == 0);
  CHECK(rename("/dev/cpuctl/g", "/dev/cpuctl/h") == 0);
  CHECK(group_of("self", "cpu", value, sizeof value) == 0 && strcmp(value, "/h") == 0);
  CHECK(put("/dev/cpuctl/cgroup.procs", pid) == 0);
  CHECK(rmdir("/dev/cpuctl/h") == 0);
}

// The unified core has one process membership, and no pretend freezer.
static void unified(void) {
  CHECK(mkdir("/sys/fs/cgroup/a", 0755) == 0);
  CHECK(mkdir("/sys/fs/cgroup/b", 0755) == 0);
  char value[256], pid[16];
  snprintf(pid, sizeof pid, "%d", getpid());
  CHECK(get("/sys/fs/cgroup/a/cgroup.controllers", value, sizeof value) == 0 && !*value);
  CHECK(put("/sys/fs/cgroup/a/cgroup.subtree_control", "+cpu") < 0 && errno == ENOENT);
  CHECK(put("/sys/fs/cgroup/a/cgroup.subtree_control", "cpu") < 0 && errno == EINVAL);
  CHECK(open("/sys/fs/cgroup/a/cgroup.freeze", O_WRONLY) < 0 && errno == ENOENT);
  CHECK(open("/sys/fs/cgroup/a/cgroup.freeze", O_WRONLY | O_CREAT, 0644) < 0 && errno == EACCES);
  CHECK(chmod("/sys/fs/cgroup/a/cgroup.procs", 0666) == 0);
  int inherited = open("/sys/fs/cgroup/a/cgroup.procs", O_WRONLY);
  CHECK(inherited >= 0);
  FORK_OR_SKIP(delegate);
  if (delegate == 0) {
    if (setuid(2000) != 0) _exit(1);
    // Target is writable, but the common ancestor is not delegated.
    if (put("/sys/fs/cgroup/a/cgroup.procs", "0") >= 0 || errno != EACCES) _exit(2);
    // An inherited file checks its root opener's saved credentials.
    if (write(inherited, "0", 1) != 1) _exit(3);
    _exit(0);
  }
  int delegated_status;
  CHECK(waitpid(delegate, &delegated_status, 0) == delegate &&
        WIFEXITED(delegated_status) && WEXITSTATUS(delegated_status) == 0);
  close(inherited);
  CHECK(rename("/sys/fs/cgroup/a", "/sys/fs/cgroup/c") < 0 && errno == EIO);
  CHECK(put("/sys/fs/cgroup/a/cgroup.procs", pid) == 0);
  CHECK(group_of("self", "", value, sizeof value) == 0 && strcmp(value, "/a") == 0);
  CHECK(lists("/sys/fs/cgroup/a/cgroup.procs", getpid()));
  CHECK(get("/sys/fs/cgroup/a/cgroup.events", value, sizeof value) == 0 &&
        strcmp(value, "populated 1\n") == 0);
  CHECK(put("/sys/fs/cgroup/b/cgroup.procs", "0") == 0);
  CHECK(!lists("/sys/fs/cgroup/a/cgroup.procs", getpid()));
  CHECK(get("/sys/fs/cgroup/a/cgroup.events", value, sizeof value) == 0 &&
        strcmp(value, "populated 0\n") == 0);
  CHECK(rmdir("/sys/fs/cgroup/b") < 0 && errno == EBUSY);
  CHECK(put("/sys/fs/cgroup/cgroup.procs", "0") == 0);
  CHECK(rmdir("/sys/fs/cgroup/a") == 0 && rmdir("/sys/fs/cgroup/b") == 0);
}

static void* idle(void* arg) {
  *(long*)arg = syscall(SYS_gettid);
  for (;;) pause();
  return NULL;
}

// After execve the new program's threads are in its main thread's group,
// whatever group a thread of the old program was moved to alone.
static int after_exec(void) {
  char seen[64];
  pthread_t t;
  if (pthread_create(&t, NULL, report, seen) != 0 || pthread_join(t, NULL) != 0) return 1;
  char main_group[64];
  if (group_of("self", "cpuset", main_group, sizeof main_group) != 0) return 1;
  return strcmp(seen, main_group) == 0 && strcmp(seen, "/") == 0 ? 0 : 1;
}

static const char* program;

static void exec_drops_threads(void) {
  char value[64];
  CHECK(mkdir("/dev/cpuset/ex", 0755) == 0);
  CHECK(get("/dev/cpuset/cpus", value, sizeof value) == 0);
  CHECK(put("/dev/cpuset/ex/cpus", value) == 0 && put("/dev/cpuset/ex/mems", "0") == 0);
  FORK_OR_SKIP(child);
  if (child == 0) {
    volatile long tid = 0;
    pthread_t t;
    if (pthread_create(&t, NULL, idle, (void*)&tid) != 0) _exit(2);
    while (!tid) usleep(1000);
    char id[16];
    snprintf(id, sizeof id, "%ld", tid);
    if (put("/dev/cpuset/ex/tasks", id) != 0) _exit(3);
    execl(program, program, "after-exec", (char*)NULL);
    _exit(4);
  }
  int status;
  CHECK(waitpid(child, &status, 0) == child && WIFEXITED(status) && WEXITSTATUS(status) == 0);
  CHECK(rmdir("/dev/cpuset/ex") == 0);
}

static volatile pid_t split_tid = -1;
static int split_go[2], split_report[2];

static int split_worker(void* arg) {
  (void)arg;
  char command, value[64];
  while (read(split_go[0], &command, 1) == 1 && command != 'q') {
    int ok = group_of("thread-self", "cpu", value, sizeof value) == 0 &&
             strcmp(value, command == 'b' ? "/split" : "/") == 0;
    if (write(split_report[1], &ok, sizeof ok) != sizeof ok) _exit(2);
  }
  return 0;
}

// cgroup.procs moves the entire Linux thread group, including a debuggerd
// pseudothread whose file table lives in a separate Darwin process.
static void split_thread_group(void) {
  CHECK(mkdir("/dev/cpuctl/split", 0755) == 0);
  CHECK(pipe(split_go) == 0 && pipe(split_report) == 0);
  static char stack[64 * 1024];
  pid_t tid = clone(split_worker, stack + sizeof stack,
                  CLONE_THREAD | CLONE_SIGHAND | CLONE_VM | CLONE_CHILD_SETTID |
                      CLONE_CHILD_CLEARTID,
                  NULL, NULL, NULL, (pid_t*)&split_tid);
  CHECK(tid > 0);
  while (split_tid == -1) syscall(SYS_futex, &split_tid, FUTEX_WAIT, -1, NULL, NULL, 0);
  CHECK(put("/dev/cpuctl/split/cgroup.procs", "0") == 0);
  int ok;
  CHECK(write(split_go[1], "b", 1) == 1 && read(split_report[0], &ok, sizeof ok) == sizeof ok && ok);
  CHECK(put("/dev/cpuctl/cgroup.procs", "0") == 0);
  CHECK(write(split_go[1], "r", 1) == 1 && read(split_report[0], &ok, sizeof ok) == sizeof ok && ok);
  CHECK(write(split_go[1], "q", 1) == 1);
  while (split_tid != 0) syscall(SYS_futex, &split_tid, FUTEX_WAIT, split_tid, NULL, NULL, 0);
  for (int i = 0; i < 2; i++) { close(split_go[i]); close(split_report[i]); }
  CHECK(rmdir("/dev/cpuctl/split") == 0);
}

static volatile int churning;

static void* nothing(void* arg) { return arg; }

static void* churn(void* arg) {
  while (churning) {
    pthread_t t;
    if (pthread_create(&t, NULL, nothing, NULL) == 0) pthread_join(t, NULL);
  }
  return arg;
}

// Every fd /proc/self/fd lists is one the process can fstat (zygote restats
// them all before each fork), while threads come and go and so the groups'
// membership is read and written.
static void membership_files_unlisted(void) {
  pthread_t t;
  churning = 1;
  CHECK(pthread_create(&t, NULL, churn, NULL) == 0);
  int unstattable = 0;
  for (int i = 0; i < 500; i++) {
    DIR* d = opendir("/proc/self/fd");
    CHECK(d != NULL);
    struct dirent* e;
    struct stat st;
    while ((e = readdir(d)) != NULL) {
      int fd = atoi(e->d_name);
      if (e->d_name[0] != '.' && fd != dirfd(d) && fstat(fd, &st) != 0) unstattable++;
    }
    closedir(d);
  }
  churning = 0;
  CHECK(pthread_join(t, NULL) == 0);
  CHECK(unstattable == 0);
}

int main(int argc, char** argv) {
  if (argc > 1 && strcmp(argv[1], "after-exec") == 0) return after_exec();
  program = argv[0];
  RUN(mounts);
  RUN(moves);
  RUN(values);
  RUN(unified);
  RUN(exec_drops_threads);
  RUN(split_thread_group);
  RUN(membership_files_unlisted);
  DONE();
}
