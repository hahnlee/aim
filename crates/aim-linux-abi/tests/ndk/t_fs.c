// File syscalls: pipes, dup3, fcntl, stat/statx, getdents64, *at calls,
// renameat2, fallocate, copies (splice through a pipe), memfd seals,
// O_TMPFILE and inotify.
// argv[1] is a writable directory.
#include <dirent.h>
#include <fcntl.h>
#include <poll.h>
#include <stdint.h>
#include <stdlib.h>
#include <sys/epoll.h>
#include <sys/file.h>
#include <sys/inotify.h>
#include <sys/mman.h>
#include <sys/sendfile.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/uio.h>
#include <sys/wait.h>
#include <unistd.h>

#include "check.h"

static char base[256];

static const char* p(const char* name) {
  static char buf[4][512];
  static int n;
  char* b = buf[n++ % 4];
  snprintf(b, 512, "%s/%s", base, name);
  return b;
}

static int make(const char* name, const char* data) {
  int fd = open(p(name), O_RDWR | O_CREAT | O_TRUNC | O_CLOEXEC, 0644);
  if (fd >= 0 && data) write(fd, data, strlen(data));
  return fd;
}

static void pipes_and_dup(void) {
  int fds[2];
  CHECK(pipe2(fds, O_CLOEXEC | O_NONBLOCK) == 0);
  CHECK((fcntl(fds[0], F_GETFD) & FD_CLOEXEC) && (fcntl(fds[1], F_GETFL) & O_NONBLOCK));
  char c;
  CHECK(read(fds[0], &c, 1) == -1 && errno == EAGAIN);
  CHECK(fcntl(fds[1], F_SETPIPE_SZ, 1 << 20) >= (1 << 20) || errno == EPERM);
  CHECK(fcntl(fds[1], F_GETPIPE_SZ) > 0);
  int d = dup3(fds[1], 100, O_CLOEXEC);
  CHECK(d == 100 && (fcntl(100, F_GETFD) & FD_CLOEXEC));
  CHECK(dup3(100, 100, 0) == -1 && errno == EINVAL);
  int e = fcntl(fds[0], F_DUPFD_CLOEXEC, 200);
  CHECK(e >= 200 && (fcntl(e, F_GETFD) & FD_CLOEXEC));
  CHECK(write(100, "p", 1) == 1 && read(e, &c, 1) == 1 && c == 'p');
  close(e);
  close(d);
  close(fds[0]);
  close(fds[1]);
}

static void stat_layout(void) {
  int fd = make("stat.txt", "12345");
  CHECK(fd >= 0);
  struct stat st;
  CHECK(fstat(fd, &st) == 0 && S_ISREG(st.st_mode) && st.st_size == 5 && st.st_nlink == 1);
  struct stat st2;
  CHECK(stat(p("stat.txt"), &st2) == 0 && st2.st_ino == st.st_ino && st2.st_dev == st.st_dev);
  CHECK(fstatat(fd, "", &st2, AT_EMPTY_PATH) == 0 && st2.st_ino == st.st_ino);
  CHECK(symlink("stat.txt", p("stat.lnk")) == 0 || errno == EEXIST);
  CHECK(lstat(p("stat.lnk"), &st2) == 0 && S_ISLNK(st2.st_mode));
  CHECK(stat(p("stat.lnk"), &st2) == 0 && st2.st_ino == st.st_ino);
  char link[64];
  ssize_t n = readlinkat(AT_FDCWD, p("stat.lnk"), link, sizeof link);
  CHECK(n == 8 && memcmp(link, "stat.txt", 8) == 0);
  struct statx sx;
  CHECK(statx(AT_FDCWD, p("stat.txt"), 0, STATX_BASIC_STATS, &sx) == 0);
  CHECK(sx.stx_size == 5 && sx.stx_ino == st.st_ino && S_ISREG(sx.stx_mode));
  CHECK((sx.stx_mask & STATX_BASIC_STATS) == STATX_BASIC_STATS);
  CHECK(sx.stx_mtime.tv_sec == st.st_mtim.tv_sec);
  CHECK(stat("/system/bin/sh", &st2) == 0);
  CHECK(stat(p("missing"), &st2) == -1 && errno == ENOENT);
  close(fd);
  unlink(p("stat.lnk"));
  unlink(p("stat.txt"));
}

static void getdents_types(void) {
  CHECK(mkdir(p("d"), 0755) == 0 || errno == EEXIST);
  close(make("d/file", "x"));
  mkdir(p("d/sub"), 0755);
  symlink("file", p("d/link"));
  DIR* d = opendir(p("d"));
  CHECK(d != NULL);
  int seen = 0;
  struct dirent* e;
  while ((e = readdir(d)) != NULL) {
    if (!strcmp(e->d_name, "file")) seen |= e->d_type == DT_REG ? 1 : 0x100;
    if (!strcmp(e->d_name, "sub")) seen |= e->d_type == DT_DIR ? 2 : 0x100;
    if (!strcmp(e->d_name, "link")) seen |= e->d_type == DT_LNK ? 4 : 0x100;
    if (!strcmp(e->d_name, ".")) seen |= 8;
  }
  CHECK(seen == 15);
  rewinddir(d);
  int count = 0;
  while (readdir(d)) count++;
  CHECK(count == 5);
  closedir(d);
  // A tiny buffer still makes progress, one entry at a time.
  int fd = open(p("d"), O_RDONLY | O_DIRECTORY);
  char buf[40];
  int entries = 0;
  long n;
  while ((n = syscall(SYS_getdents64, fd, buf, sizeof buf)) > 0) entries++;
  CHECK(n == 0 && entries == 5);
  close(fd);
  // The image's directories list too.
  d = opendir("/system/bin");
  CHECK(d != NULL);
  count = 0;
  while (readdir(d)) count++;
  CHECK(count > 100);
  closedir(d);
}

static void at_calls(void) {
  int dfd = open(base, O_RDONLY | O_DIRECTORY);
  CHECK(dfd >= 0);
  CHECK(mkdirat(dfd, "at", 0700) == 0 || errno == EEXIST);
  CHECK(unlinkat(dfd, "at", 0) == -1 && errno == EISDIR);
  CHECK(symlinkat("at", dfd, "at.lnk") == 0);
  CHECK(unlinkat(dfd, "at.lnk", AT_REMOVEDIR) == -1 && errno == ENOTDIR);
  CHECK(unlinkat(dfd, "at.lnk", 0) == 0);
  int fd = openat(dfd, "at/f", O_CREAT | O_WRONLY | O_TRUNC, 0600);
  CHECK(fd >= 0);
  CHECK(faccessat(dfd, "at/f", R_OK | W_OK, 0) == 0);
  CHECK(syscall(SYS_faccessat2, dfd, "at/f", R_OK, AT_EACCESS) == 0);
  CHECK(faccessat(dfd, "at/none", F_OK, 0) == -1 && errno == ENOENT);
  CHECK(fchmodat(dfd, "at/f", 0640, 0) == 0);
  struct stat st;
  CHECK(fstatat(dfd, "at/f", &st, 0) == 0 && (st.st_mode & 0777) == 0640);
  CHECK(linkat(dfd, "at/f", dfd, "at/g", 0) == 0);
  CHECK(fstatat(dfd, "at/f", &st, 0) == 0 && st.st_nlink == 2);
  struct timespec ts[2] = {{1000000000, 0}, {1000000000, 500}};
  CHECK(utimensat(dfd, "at/f", ts, 0) == 0);
  CHECK(fstatat(dfd, "at/f", &st, 0) == 0 && st.st_mtim.tv_sec == 1000000000);
  struct timespec omit[2] = {{0, UTIME_NOW}, {0, UTIME_OMIT}};
  CHECK(utimensat(dfd, "at/f", omit, 0) == 0);
  CHECK(fstatat(dfd, "at/f", &st, 0) == 0 && st.st_mtim.tv_sec == 1000000000 && st.st_atim.tv_sec > 1000000000);
  CHECK(unlinkat(dfd, "at", 0) == -1 && errno == EISDIR);
  CHECK(unlinkat(dfd, "at/f", 1) == -1 && errno == EINVAL);
  CHECK(unlinkat(dfd, "at/g", 0) == 0);
  CHECK(unlinkat(dfd, "at", AT_REMOVEDIR) == -1 && errno == ENOTEMPTY);
  CHECK(unlinkat(dfd, "at/f", 0) == 0 && unlinkat(dfd, "at", AT_REMOVEDIR) == 0);
  CHECK(unlinkat(dfd, "at", 0) == -1 && errno == ENOENT);
  CHECK(mkdirat(dfd, "at", 0700) == 0);
  CHECK(remove(p("at")) == 0);
  // The image is read-only under a path map.
  CHECK(open("/system/build.prop", O_RDWR) == -1 && errno == EROFS);
  CHECK(mkdir("/system/new-dir", 0755) == -1 && errno == EROFS);
  CHECK(access("/system/build.prop", W_OK) == -1 && errno == EROFS);
  close(fd);
  close(dfd);
}

// A fork child knows the owners its parent read, and sees a later chown
// or chmod by either process, as every process sees one inode.
static void owners_across_fork(void) {
  int fd = make("owned", "x");
  CHECK(fd >= 0);
  close(fd);
  CHECK(chown(p("owned"), 10123, 10124) == 0 && chmod(p("owned"), 0640) == 0);
  struct stat st;
  CHECK(stat(p("owned"), &st) == 0 && st.st_uid == 10123 && (st.st_mode & 0777) == 0640);
  int go[2], done[2];
  CHECK(pipe(go) == 0 && pipe(done) == 0);
  fflush(stdout);
  FORK_OR_SKIP(c);
  if (c == 0) {
    char b;
    if (stat(p("owned"), &st) != 0 || st.st_uid != 10123 || st.st_gid != 10124 ||
        (st.st_mode & 0777) != 0640)
      _exit(1);
    if (write(done[1], "r", 1) != 1 || read(go[0], &b, 1) != 1) _exit(2);
    if (stat(p("owned"), &st) != 0 || (st.st_mode & 0777) != 0600 || st.st_uid != 2000) _exit(3);
    _exit(chmod(p("owned"), 0604) == 0 ? 0 : 4);
  }
  char b;
  CHECK(read(done[0], &b, 1) == 1);
  CHECK(chmod(p("owned"), 0600) == 0 && chown(p("owned"), 2000, -1) == 0);
  CHECK(write(go[1], "g", 1) == 1);
  int status;
  CHECK(waitpid(c, &status, 0) == c && WIFEXITED(status));
  if (WEXITSTATUS(status)) printf("child failed at step %d\n", WEXITSTATUS(status));
  CHECK(WEXITSTATUS(status) == 0);
  CHECK(stat(p("owned"), &st) == 0 && (st.st_mode & 0777) == 0604 && st.st_uid == 2000);
  close(go[0]);
  close(go[1]);
  close(done[0]);
  close(done[1]);
  unlink(p("owned"));
}

static void rename2(void) {
  close(make("r1", "one"));
  close(make("r2", "two"));
  CHECK(renameat2(AT_FDCWD, p("r1"), AT_FDCWD, p("r2"), RENAME_NOREPLACE) == -1 && errno == EEXIST);
  CHECK(renameat2(AT_FDCWD, p("r1"), AT_FDCWD, p("r2"), RENAME_EXCHANGE) == 0);
  char b[4] = {0};
  int fd = open(p("r1"), O_RDONLY);
  CHECK(read(fd, b, 3) == 3 && !strcmp(b, "two"));
  close(fd);
  CHECK(rename(p("r1"), p("r3")) == 0);
  CHECK(access(p("r1"), F_OK) == -1 && errno == ENOENT);
  unlink(p("r2"));
  unlink(p("r3"));
}

static void sizes_and_sync(void) {
  int fd = make("size", "abc");
  CHECK(ftruncate(fd, 100000) == 0);
  struct stat st;
  CHECK(fstat(fd, &st) == 0 && st.st_size == 100000);
  CHECK(fallocate(fd, 0, 0, 200000) == 0 && fstat(fd, &st) == 0 && st.st_size == 200000);
  CHECK(fallocate(fd, FALLOC_FL_KEEP_SIZE, 0, 300000) == 0 && fstat(fd, &st) == 0 && st.st_size == 200000);
  CHECK(fallocate(fd, FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE, 0, 65536) == 0);
  char z;
  CHECK(pread(fd, &z, 1, 0) == 1 && z == 0);
  CHECK(sync_file_range(fd, 0, 0, SYNC_FILE_RANGE_WRITE) == 0);
  CHECK(fsync(fd) == 0 && fdatasync(fd) == 0);
  CHECK(truncate(p("size"), 1) == 0 && fstat(fd, &st) == 0 && st.st_size == 1);
  CHECK(flock(fd, LOCK_EX | LOCK_NB) == 0 && flock(fd, LOCK_UN) == 0);
  struct flock l = {.l_type = F_WRLCK, .l_whence = SEEK_SET, .l_start = 0, .l_len = 10};
  CHECK(fcntl(fd, F_OFD_SETLK, &l) == 0);
  // An OFD lock and a process lock have different owners: they conflict.
  CHECK(fcntl(fd, F_SETLK, &l) == -1 && errno == EAGAIN);
  l.l_type = F_UNLCK;
  CHECK(fcntl(fd, F_OFD_SETLK, &l) == 0);
  l.l_type = F_WRLCK;
  CHECK(fcntl(fd, F_SETLK, &l) == 0);
  struct flock q = {.l_type = F_WRLCK, .l_whence = SEEK_SET, .l_len = 10};
  CHECK(fcntl(fd, F_GETLK, &q) == 0 && q.l_type == F_UNLCK);
  struct iovec v[2] = {{"hel", 3}, {"lo", 2}};
  CHECK(pwritev(fd, v, 2, 10) == 5);
  char out[5];
  struct iovec r = {out, 5};
  CHECK(preadv(fd, &r, 1, 10) == 5 && memcmp(out, "hello", 5) == 0);
  close(fd);
  unlink(p("size"));
}

static void copies(void) {
  int src = make("src", "copy me please");
  int dst = make("dst", NULL);
  off_t off = 5;
  CHECK(sendfile(dst, src, &off, 4) == 4 && off == 9);
  loff_t in = 0, outo = 100;
  CHECK(copy_file_range(src, &in, dst, &outo, 4, 0) == 4 && in == 4 && outo == 104);
  char b[8] = {0};
  CHECK(pread(dst, b, 4, 0) == 4 && !memcmp(b, "me p", 4));
  CHECK(pread(dst, b, 4, 100) == 4 && !memcmp(b, "copy", 4));
  int pp[2];
  pipe(pp);
  loff_t s0 = 0;
  CHECK(splice(src, &s0, pp[1], NULL, 4, 0) == 4 && s0 == 4);
  CHECK(read(pp[0], b, 4) == 4 && !memcmp(b, "copy", 4));
  close(pp[0]);
  close(pp[1]);
  close(src);
  close(dst);
  unlink(p("src"));
  unlink(p("dst"));
}

// A splice from a socket into a pipe moves what fits at once, even when
// asked for more than the pipe holds; the caller drains the pipe after
// (FileUtils' copy, as PackageInstaller streams an APK).
static void splice_through_a_pipe(void) {
  int sv[2], pp[2];
  CHECK(socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0 && pipe(pp) == 0);
  static char data[256 << 10];
  for (size_t i = 0; i < sizeof(data); i++) data[i] = (char)(i * 7);
  FORK_OR_SKIP(pid);
  if (pid == 0) {
    close(sv[0]);
    size_t sent = 0;
    while (sent < sizeof(data)) {
      ssize_t n = write(sv[1], data + sent, sizeof(data) - sent);
      if (n <= 0) _exit(1);
      sent += n;
    }
    _exit(0);
  }
  close(sv[1]);
  alarm(20);
  int dst = make("spliced", NULL);
  size_t total = 0;
  for (;;) {
    ssize_t t = splice(sv[0], NULL, pp[1], NULL, 512 << 10, SPLICE_F_MOVE | SPLICE_F_MORE);
    CHECK(t >= 0);
    if (t == 0) break;
    while (t > 0) {
      ssize_t m = splice(pp[0], NULL, dst, NULL, t, SPLICE_F_MOVE | SPLICE_F_MORE);
      CHECK(m > 0);
      t -= m;
      total += m;
    }
  }
  alarm(0);
  CHECK(total == sizeof(data));
  static char back[256 << 10];
  CHECK(pread(dst, back, sizeof(back), 0) == (ssize_t)sizeof(back) && !memcmp(back, data, sizeof(data)));
  // A full pipe with SPLICE_F_NONBLOCK: EAGAIN instead of waiting.
  char fill[4096] = {0};
  CHECK(fcntl(pp[1], F_SETFL, O_NONBLOCK) == 0);
  while (write(pp[1], fill, sizeof(fill)) > 0) {
  }
  CHECK(fcntl(pp[1], F_SETFL, 0) == 0);
  int src = make("spliced2", "x");
  loff_t o = 0;
  CHECK(splice(src, &o, pp[1], NULL, 1, SPLICE_F_NONBLOCK) == -1 && errno == EAGAIN);
  int status;
  CHECK(waitpid(pid, &status, 0) == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0);
  close(src);
  close(dst);
  close(sv[0]);
  close(pp[0]);
  close(pp[1]);
  unlink(p("spliced"));
  unlink(p("spliced2"));
}

static void memfd_and_seals(void) {
  int fd = memfd_create("sysio", MFD_CLOEXEC | MFD_ALLOW_SEALING);
  CHECK(fd >= 0);
  CHECK(write(fd, "sealed", 6) == 6);
  char link[64] = {0};
  char path[64];
  snprintf(path, sizeof path, "/proc/self/fd/%d", fd);
  CHECK(readlink(path, link, sizeof link) > 0 && !strcmp(link, "/memfd:sysio (deleted)"));
  CHECK(ftruncate(fd, 16384) == 0);
  char* m = mmap(NULL, 16384, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
  CHECK(m != MAP_FAILED && !memcmp(m, "sealed", 6));
  m[0] = 'S';
  char b;
  CHECK(pread(fd, &b, 1, 0) == 1 && b == 'S');
  CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_SHRINK | F_SEAL_GROW) == 0);
  CHECK(ftruncate(fd, 100) == -1 && errno == EPERM);
  CHECK(ftruncate(fd, 1 << 20) == -1 && errno == EPERM);
  CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_FUTURE_WRITE) == 0);
  CHECK(write(fd, "x", 1) == -1 && errno == EPERM);
  CHECK(mmap(NULL, 16384, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0) == MAP_FAILED && errno == EPERM);
  CHECK(mmap(NULL, 16384, PROT_READ, MAP_SHARED, fd, 0) != MAP_FAILED);
  CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_SEAL) == 0);
  CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_WRITE) == -1 && errno == EPERM);
  CHECK(fcntl(fd, F_GET_SEALS) == (F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_FUTURE_WRITE | F_SEAL_SEAL));
  int plain = memfd_create("nosealing", 0);
  CHECK(fcntl(plain, F_ADD_SEALS, F_SEAL_WRITE) == -1 && errno == EPERM);
  close(plain);
  munmap(m, 16384);
  close(fd);
}

static int listed(const char* dir) {
  DIR* d = opendir(dir);
  if (!d) return -1;
  int n = 0;
  struct dirent* e;
  while ((e = readdir(d)) != NULL) n += strcmp(e->d_name, ".") && strcmp(e->d_name, "..");
  closedir(d);
  return n;
}

// O_TMPFILE: an unnamed file in a directory that linkat names later, by
// its /proc/self/fd link (as tombstoned names a tombstone) or by
// AT_EMPTY_PATH, unless it was opened O_EXCL (open(2), linkat(2)).
static void tmpfile_and_linkat(void) {
  CHECK(mkdir(p("tmp"), 0755) == 0);
  int dfd = open(p("tmp"), O_RDONLY | O_DIRECTORY | O_CLOEXEC);
  CHECK(dfd >= 0);
  CHECK(open(p("tmp"), O_TMPFILE | O_RDONLY, 0600) == -1 && errno == EINVAL);
  CHECK(open(p("tmp"), O_TMPFILE | O_CREAT | O_RDWR, 0600) == -1 && errno == EINVAL);
  close(make("tmp-file", "x"));
  CHECK(open(p("tmp-file"), O_TMPFILE | O_RDWR, 0600) == -1 && errno == ENOTDIR);
  int fd = open(p("tmp"), O_TMPFILE | O_WRONLY | O_CLOEXEC, 0640);
  CHECK(fd >= 0);
  CHECK(write(fd, "tomb", 4) == 4);
  struct stat st;
  CHECK(fstat(fd, &st) == 0 && S_ISREG(st.st_mode) && st.st_size == 4);
  CHECK(listed(p("tmp")) == 0);
  char link[64], target[512], want[512];
  snprintf(link, sizeof link, "/proc/self/fd/%d", fd);
  ssize_t n = readlink(link, target, sizeof target - 1);
  CHECK(n > 0);
  target[n] = 0;
  snprintf(want, sizeof want, "%s/#%llu (deleted)", p("tmp"), (unsigned long long)st.st_ino);
  CHECK(!strcmp(target, want));
  CHECK(linkat(AT_FDCWD, link, dfd, "named", AT_SYMLINK_FOLLOW) == 0);
  CHECK(listed(p("tmp")) == 1);
  // The name is the same file: later writes show through it.
  CHECK(write(fd, "stone", 5) == 5);
  struct stat named;
  CHECK(stat(p("tmp/named"), &named) == 0);
  CHECK(named.st_ino == st.st_ino && named.st_size == 9);
  close(fd);
  // An existing name is not replaced.
  fd = open(p("tmp"), O_TMPFILE | O_RDWR, 0600);
  CHECK(fd >= 0);
  CHECK(linkat(fd, "", dfd, "named", AT_EMPTY_PATH) == -1 && errno == EEXIST);
  CHECK(linkat(fd, "", dfd, "second", AT_EMPTY_PATH) == 0);
  CHECK(listed(p("tmp")) == 2);
  close(fd);
  // With O_EXCL it can never be linked.
  fd = open(p("tmp"), O_TMPFILE | O_RDWR | O_EXCL, 0600);
  CHECK(fd >= 0);
  CHECK(linkat(fd, "", dfd, "never", AT_EMPTY_PATH) == -1 && errno == ENOENT);
  close(fd);
  CHECK(listed(p("tmp")) == 2);
  close(dfd);
}

static void inotify_dir(void) {
  mkdir(p("watch"), 0755);
  int in = inotify_init1(IN_NONBLOCK | IN_CLOEXEC);
  CHECK(in >= 0);
  int wd = inotify_add_watch(in, p("watch"), IN_CREATE | IN_DELETE | IN_MODIFY | IN_CLOSE_WRITE);
  CHECK(wd > 0);
  char buf[4096];
  CHECK(read(in, buf, sizeof buf) == -1 && errno == EAGAIN);
  int fd = make("watch/new", "data");
  close(fd);
  struct pollfd pf = {.fd = in, .events = POLLIN};
  CHECK(poll(&pf, 1, 1000) == 1);
  ssize_t n = read(in, buf, sizeof buf);
  CHECK(n > 0);
  struct inotify_event* e = (struct inotify_event*)buf;
  CHECK(e->wd == wd && (e->mask & IN_CREATE) && e->len > 0 && !strcmp(e->name, "new"));
  unlink(p("watch/new"));
  CHECK(poll(&pf, 1, 1000) == 1);
  n = read(in, buf, sizeof buf);
  e = (struct inotify_event*)buf;
  CHECK(n > 0 && (e->mask & IN_DELETE) && !strcmp(e->name, "new"));
  CHECK(inotify_rm_watch(in, wd) == 0);
  n = read(in, buf, sizeof buf);
  e = (struct inotify_event*)buf;
  CHECK(n > 0 && (e->mask & IN_IGNORED));
  CHECK(inotify_rm_watch(in, wd) == -1 && errno == EINVAL);
  close(in);
  rmdir(p("watch"));
}

// A write into a file of a watched directory is IN_MODIFY and
// IN_CLOSE_WRITE for its name, also after the directory itself is done
// changing: the file is created empty, then written (WallpaperManagerService
// waits for IN_CLOSE_WRITE on its wallpaper file this way), and an existing
// file is written again; a file renamed away and created anew as well.
static int next_named(int in, uint32_t mask, const char* name) {
  char buf[4096];
  struct pollfd pf = {.fd = in, .events = POLLIN};
  for (int tries = 0; tries < 8 && poll(&pf, 1, 1000) == 1; tries++) {
    ssize_t n = read(in, buf, sizeof buf);
    for (char* q = buf; n > 0 && q < buf + n;) {
      struct inotify_event* e = (struct inotify_event*)q;
      if ((e->mask & mask) && e->len > 0 && !strcmp(e->name, name)) return 1;
      q += sizeof *e + e->len;
    }
  }
  return 0;
}

static void inotify_write_into_entry(void) {
  mkdir(p("written"), 0755);
  int in = inotify_init1(IN_NONBLOCK | IN_CLOEXEC);
  CHECK(in >= 0);
  CHECK(inotify_add_watch(in, p("written"), IN_CLOSE_WRITE | IN_CREATE) > 0);
  int fd = open(p("written/w"), O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0600);
  CHECK(fd >= 0);
  CHECK(next_named(in, IN_CREATE, "w"));
  CHECK(write(fd, "data", 4) == 4);
  close(fd);
  CHECK(next_named(in, IN_CLOSE_WRITE, "w"));
  fd = open(p("written/w"), O_WRONLY | O_TRUNC | O_CLOEXEC);
  CHECK(fd >= 0 && write(fd, "again", 5) == 5);
  close(fd);
  CHECK(next_named(in, IN_CLOSE_WRITE, "w"));
  CHECK(rename(p("written/w"), p("written/old")) == 0);
  fd = open(p("written/w"), O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0600);
  CHECK(fd >= 0);
  CHECK(next_named(in, IN_CREATE, "w"));
  CHECK(write(fd, "new", 3) == 3);
  close(fd);
  CHECK(next_named(in, IN_CLOSE_WRITE, "w"));
  close(in);
  unlink(p("written/w"));
  unlink(p("written/old"));
  rmdir(p("written"));
}

// A change the watch does not ask for queues nothing, so poll and epoll do
// not report the fd readable (a blocking read would wait for an event).
static void inotify_unwatched_changes(void) {
  mkdir(p("quiet"), 0755);
  int in = inotify_init1(IN_CLOEXEC);
  CHECK(in >= 0);
  CHECK(inotify_add_watch(in, p("quiet"), IN_CREATE) > 0);
  CHECK(chmod(p("quiet"), 0700) == 0);
  struct pollfd pf = {.fd = in, .events = POLLIN};
  CHECK(poll(&pf, 1, 300) == 0);
  int ep = epoll_create1(EPOLL_CLOEXEC);
  struct epoll_event ev = {.events = EPOLLIN, .data.fd = in};
  CHECK(epoll_ctl(ep, EPOLL_CTL_ADD, in, &ev) == 0);
  CHECK(chmod(p("quiet"), 0755) == 0);
  CHECK(epoll_wait(ep, &ev, 1, 300) == 0);
  int fd = make("quiet/new", "data");
  close(fd);
  CHECK(epoll_wait(ep, &ev, 1, 1000) == 1 && ev.events == EPOLLIN);
  char buf[256];
  CHECK(read(in, buf, sizeof buf) > 0);
  close(ep);
  close(in);
  unlink(p("quiet/new"));
  rmdir(p("quiet"));
}

int main(int argc, char** argv) {
  snprintf(base, sizeof base, "%s/fs-%d", argc > 1 ? argv[1] : "/data/local/tmp", getpid());
  mkdir(base, 0755);
  RUN(pipes_and_dup);
  RUN(stat_layout);
  RUN(getdents_types);
  RUN(at_calls);
  RUN(owners_across_fork);
  RUN(rename2);
  RUN(sizes_and_sync);
  RUN(copies);
  RUN(splice_through_a_pipe);
  RUN(memfd_and_seals);
  RUN(tmpfile_and_linkat);
  RUN(inotify_dir);
  RUN(inotify_unwatched_changes);
  RUN(inotify_write_into_entry);
  DONE();
}
