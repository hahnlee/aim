// Evdev devices (/dev/input/eventN, docs/input.md), against the Linux UAPI
// headers: the display server's touchscreen (event0), keyboard (event1,
// KEY_A held by the harness) and mouse (event2).
#include <dirent.h>
#include <fcntl.h>
#include <limits.h>
#include <linux/input.h>
#include <poll.h>
#include <stdint.h>
#include <stdlib.h>
#include <sys/epoll.h>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <sys/sysmacros.h>
#include <sys/time.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#include "check.h"

#define TEST_BIT(bits, n) (((bits)[(n) / 8] >> ((n) % 8)) & 1)

static int64_t now_ns(clockid_t id) {
  struct timespec t;
  clock_gettime(id, &t);
  return t.tv_sec * 1000000000LL + t.tv_nsec;
}

static int64_t event_ns(const struct input_event* e) {
  return e->input_event_sec * 1000000000LL + e->input_event_usec * 1000LL;
}

static void identity(void) {
  int fd = open("/dev/input/event0", O_RDONLY | O_NONBLOCK | O_CLOEXEC);
  CHECK(fd >= 0);
  int version = 0;
  CHECK(ioctl(fd, EVIOCGVERSION, &version) == 0 && version == EV_VERSION);
  struct input_id id;
  CHECK(ioctl(fd, EVIOCGID, &id) == 0 && id.bustype == BUS_VIRTUAL);
  char name[80];
  // The name and its NUL, or as much as fits.
  CHECK(ioctl(fd, EVIOCGNAME(sizeof(name)), name) == 16);
  CHECK(strcmp(name, "aim-touchscreen") == 0);
  CHECK(ioctl(fd, EVIOCGNAME(4), name) == 4 && memcmp(name, "aim-", 4) == 0);
  CHECK(ioctl(fd, EVIOCGPHYS(sizeof(name)), name) == -1 && errno == ENOENT);
  CHECK(ioctl(fd, EVIOCGUNIQ(sizeof(name)), name) == -1 && errno == ENOENT);
  // Not an evdev request: the generic answer.
  CHECK(ioctl(fd, TCGETS, name) == -1 && errno == ENOTTY);
  close(fd);
}

static void capabilities(void) {
  int fd = open("/dev/input/event0", O_RDONLY | O_NONBLOCK);
  CHECK(fd >= 0);
  uint8_t bits[256];
  memset(bits, 0, sizeof(bits));
  // Bitmaps are as long as the kernel's: BITS_TO_LONGS(max) longs.
  CHECK(ioctl(fd, EVIOCGBIT(0, sizeof(bits)), bits) == 8);
  CHECK(TEST_BIT(bits, EV_SYN) && TEST_BIT(bits, EV_KEY) && TEST_BIT(bits, EV_ABS));
  CHECK(!TEST_BIT(bits, EV_REL) && !TEST_BIT(bits, EV_REP));
  CHECK(ioctl(fd, EVIOCGBIT(EV_KEY, sizeof(bits)), bits) == (KEY_MAX + 63) / 64 * 8);
  CHECK(TEST_BIT(bits, BTN_TOUCH));
  CHECK(ioctl(fd, EVIOCGBIT(EV_KEY, 10), bits) == 10);
  CHECK(ioctl(fd, EVIOCGBIT(EV_ABS, sizeof(bits)), bits) == 8);
  CHECK(TEST_BIT(bits, ABS_MT_SLOT) && TEST_BIT(bits, ABS_MT_POSITION_X) &&
        TEST_BIT(bits, ABS_MT_POSITION_Y) && TEST_BIT(bits, ABS_MT_TRACKING_ID));
  CHECK(ioctl(fd, EVIOCGBIT(EV_FF, sizeof(bits)), bits) == 16);
  CHECK(ioctl(fd, EVIOCGBIT(EV_REP, sizeof(bits)), bits) == -1 && errno == EINVAL);
  CHECK(ioctl(fd, EVIOCGPROP(sizeof(bits)), bits) == 8 && TEST_BIT(bits, INPUT_PROP_DIRECT));
  struct input_absinfo abs;
  CHECK(ioctl(fd, EVIOCGABS(ABS_MT_POSITION_X), &abs) == 0);
  CHECK(abs.minimum == 0 && abs.maximum == 1079 && abs.resolution == 10);
  CHECK(ioctl(fd, EVIOCGABS(ABS_MT_SLOT), &abs) == 0 && abs.maximum == 9 && abs.value == 0);
  unsigned rep[2];
  CHECK(ioctl(fd, EVIOCGREP, rep) == -1 && errno == ENOSYS);
  int effects = -1;
  CHECK(ioctl(fd, EVIOCGEFFECTS, &effects) == 0 && effects == 0);
  CHECK(ioctl(fd, EVIOCRMFF, 0) == -1 && errno == ENOSYS);
  struct {
    uint32_t code;
    int32_t values[16];
  } mt;
  mt.code = ABS_MT_TRACKING_ID;
  CHECK(ioctl(fd, EVIOCGMTSLOTS(sizeof(mt)), &mt) == 0);
  for (int i = 0; i < 10; i++) CHECK(mt.values[i] == -1);
  mt.code = ABS_X;
  CHECK(ioctl(fd, EVIOCGMTSLOTS(sizeof(mt)), &mt) == -1 && errno == EINVAL);
  close(fd);

  fd = open("/dev/input/event1", O_RDONLY | O_NONBLOCK);
  CHECK(fd >= 0);
  CHECK(ioctl(fd, EVIOCGABS(ABS_X), &abs) == -1 && errno == EINVAL);
  mt.code = ABS_MT_TRACKING_ID;
  CHECK(ioctl(fd, EVIOCGMTSLOTS(sizeof(mt)), &mt) == -1 && errno == EINVAL);
  // The key the harness holds.
  memset(bits, 0, sizeof(bits));
  CHECK(ioctl(fd, EVIOCGKEY(sizeof(bits)), bits) == (KEY_MAX + 63) / 64 * 8);
  CHECK(TEST_BIT(bits, KEY_A) && !TEST_BIT(bits, KEY_B));
  CHECK(ioctl(fd, EVIOCGSW(sizeof(bits)), bits) == 8);
  close(fd);
}

static void reads(void) {
  int fd = open("/dev/input/event1", O_RDWR | O_NONBLOCK);
  CHECK(fd >= 0);
  struct input_event ev[8];
  CHECK(read(fd, ev, 10) == -1 && errno == EINVAL);
  CHECK(read(fd, ev, sizeof(ev)) == -1 && errno == EAGAIN);
  CHECK(read(fd, ev, 0) == -1 && errno == EAGAIN);
  CHECK(write(fd, ev, 10) == -1 && errno == EINVAL);
  int ro = open("/dev/input/event1", O_RDONLY);
  CHECK(ro >= 0);
  CHECK(write(ro, ev, sizeof(ev[0])) == -1 && errno == EBADF);
  int clk = CLOCK_PROCESS_CPUTIME_ID;
  CHECK(ioctl(ro, EVIOCSCLOCKID, &clk) == -1 && errno == EINVAL);
  clk = CLOCK_MONOTONIC;
  CHECK(ioctl(ro, EVIOCSCLOCKID, &clk) == 0);

  // Written events are injected into the device and reach every reader,
  // stamped when the device reports them.
  struct input_event in[2];
  memset(in, 0, sizeof(in));
  in[0].type = EV_KEY;
  in[0].code = KEY_B;
  in[0].value = 1;
  in[1].type = EV_SYN;
  in[1].code = SYN_REPORT;
  int64_t before_mono = now_ns(CLOCK_MONOTONIC), before_real = now_ns(CLOCK_REALTIME);
  CHECK(write(fd, in, sizeof(in)) == sizeof(in));
  int ep = epoll_create1(EPOLL_CLOEXEC);
  struct epoll_event e = {.events = EPOLLIN, .data.fd = ro};
  CHECK(epoll_ctl(ep, EPOLL_CTL_ADD, ro, &e) == 0);
  CHECK(epoll_wait(ep, &e, 1, 5000) == 1 && e.events == EPOLLIN);
  // Blocking read, room for more than there is.
  CHECK(read(ro, ev, sizeof(ev)) == 2 * sizeof(ev[0]));
  CHECK(ev[0].type == EV_KEY && ev[0].code == KEY_B && ev[0].value == 1);
  CHECK(ev[1].type == EV_SYN && ev[1].code == SYN_REPORT);
  CHECK(event_ns(&ev[0]) == event_ns(&ev[1]));
  CHECK(event_ns(&ev[0]) >= before_mono - 1000 && event_ns(&ev[0]) <= now_ns(CLOCK_MONOTONIC));
  // The default clock is CLOCK_REALTIME.
  struct pollfd p = {.fd = fd, .events = POLLIN};
  CHECK(poll(&p, 1, 5000) == 1 && p.revents == POLLIN);
  CHECK(read(fd, ev, sizeof(ev[0])) == sizeof(ev[0]) && ev[0].code == KEY_B);
  CHECK(event_ns(&ev[0]) >= before_real - 1000 && event_ns(&ev[0]) <= now_ns(CLOCK_REALTIME));
  // The rest of the packet, then nothing.
  CHECK(read(fd, ev, sizeof(ev)) == sizeof(ev[0]) && ev[0].type == EV_SYN);
  CHECK(read(fd, ev, sizeof(ev)) == -1 && errno == EAGAIN);
  // State follows what was read.
  uint8_t bits[96];
  CHECK(ioctl(ro, EVIOCGKEY(sizeof(bits)), bits) == 96 && TEST_BIT(bits, KEY_B));
  // Unchanged values and undeclared codes are dropped: nothing to read.
  in[0].code = KEY_B;
  CHECK(write(fd, in, sizeof(in)) == sizeof(in));
  in[0].code = BTN_TOUCH;
  CHECK(write(fd, in, sizeof(in)) == sizeof(in));
  p.fd = ro;
  CHECK(poll(&p, 1, 300) == 0);
  in[0].code = KEY_B;
  in[0].value = 0;
  CHECK(write(fd, in, sizeof(in)) == sizeof(in));
  CHECK(read(ro, ev, sizeof(ev)) == 2 * sizeof(ev[0]) && ev[0].value == 0);
  close(ep);
  close(ro);
  close(fd);
}

static void grab_and_revoke(void) {
  int a = open("/dev/input/event2", O_RDWR | O_NONBLOCK);
  int b = open("/dev/input/event2", O_RDONLY | O_NONBLOCK);
  CHECK(a >= 0 && b >= 0);
  CHECK(ioctl(a, EVIOCGRAB, 1) == 0);
  CHECK(ioctl(b, EVIOCGRAB, 1) == -1 && errno == EBUSY);
  CHECK(ioctl(a, EVIOCGRAB, 1) == -1 && errno == EBUSY);
  struct input_event in[2];
  memset(in, 0, sizeof(in));
  in[0].type = EV_REL;
  in[0].code = REL_WHEEL;
  in[0].value = 1;
  in[1].type = EV_SYN;
  CHECK(write(a, in, sizeof(in)) == sizeof(in));
  struct pollfd p = {.fd = a, .events = POLLIN};
  CHECK(poll(&p, 1, 5000) == 1);
  p.fd = b;
  CHECK(poll(&p, 1, 300) == 0);
  CHECK(ioctl(b, EVIOCGRAB, 0) == -1 && errno == EINVAL);
  CHECK(ioctl(a, EVIOCGRAB, 0) == 0);
  // Revoked: no more reads or requests, and poll says hung up.
  CHECK(ioctl(a, EVIOCREVOKE, 1) == -1 && errno == EINVAL);
  CHECK(ioctl(a, EVIOCREVOKE, 0) == 0);
  struct input_event ev;
  CHECK(read(a, &ev, sizeof(ev)) == -1 && errno == ENODEV);
  int version;
  CHECK(ioctl(a, EVIOCGVERSION, &version) == -1 && errno == ENODEV);
  p.fd = a;
  CHECK(poll(&p, 1, 1000) == 1 && (p.revents & POLLHUP));
  close(a);
  close(b);
}

static void key(int fd, int code, int value) {
  struct input_event in[2];
  memset(in, 0, sizeof(in));
  in[0].type = EV_KEY;
  in[0].code = code;
  in[0].value = value;
  in[1].type = EV_SYN;
  in[1].code = SYN_REPORT;
  write(fd, in, sizeof(in));
}

// Read what is queued on `fd` (non-blocking), waiting up to a second for
// the first event.
static int drain(int fd, struct input_event* ev, int max) {
  struct pollfd p = {.fd = fd, .events = POLLIN};
  if (poll(&p, 1, 1000) != 1) return 0;
  usleep(50000);
  ssize_t n = read(fd, ev, max * sizeof(*ev));
  return n < 0 ? 0 : n / sizeof(*ev);
}

static void masks(void) {
  int a = open("/dev/input/event1", O_RDWR | O_NONBLOCK);
  int b = open("/dev/input/event1", O_RDONLY | O_NONBLOCK);
  CHECK(a >= 0 && b >= 0);
  uint8_t codes[96];
  struct input_mask m = {.type = EV_KEY, .codes_size = sizeof(codes), .codes_ptr = (uintptr_t)codes};
  // No mask yet: every code passes.
  memset(codes, 0, sizeof(codes));
  CHECK(ioctl(b, EVIOCGMASK, &m) == 0 && codes[0] == 0xff && codes[95] == 0xff);
  // KEY_C alone passes.
  memset(codes, 0, sizeof(codes));
  codes[KEY_C / 8] |= 1 << (KEY_C % 8);
  CHECK(ioctl(b, EVIOCSMASK, &m) == 0);
  memset(codes, 0xaa, sizeof(codes));
  CHECK(ioctl(b, EVIOCGMASK, &m) == 0 && TEST_BIT(codes, KEY_C) && !TEST_BIT(codes, KEY_D));
  // Beyond the kernel's bitmap the buffer is zeroed; a type without codes
  // to mask reads zeroes and takes any mask.
  uint8_t big[128];
  memset(big, 0xaa, sizeof(big));
  struct input_mask m2 = {.type = EV_KEY, .codes_size = sizeof(big), .codes_ptr = (uintptr_t)big};
  CHECK(ioctl(b, EVIOCGMASK, &m2) == 0 && TEST_BIT(big, KEY_C) && big[96] == 0 && big[127] == 0);
  m2.type = EV_REP;
  memset(big, 0xaa, sizeof(big));
  CHECK(ioctl(b, EVIOCGMASK, &m2) == 0 && big[0] == 0 && big[127] == 0);
  CHECK(ioctl(b, EVIOCSMASK, &m2) == 0);
  m2.codes_ptr = 0;
  CHECK(ioctl(b, EVIOCGMASK, &m2) == -1 && errno == EFAULT);

  // KEY_D's packet never reaches b; KEY_C's does.
  key(a, KEY_D, 1);
  key(a, KEY_C, 1);
  struct input_event ev[8];
  CHECK(drain(a, ev, 8) == 4);
  CHECK(drain(b, ev, 8) == 2 && ev[0].code == KEY_C && ev[1].type == EV_SYN);
  CHECK(read(b, ev, sizeof(ev)) == -1 && errno == EAGAIN);
  // The state still has KEY_D: masked codes answer from the device.
  uint8_t bits[96];
  CHECK(ioctl(b, EVIOCGKEY(sizeof(bits)), bits) == 96 && TEST_BIT(bits, KEY_D) &&
        TEST_BIT(bits, KEY_C) && TEST_BIT(bits, KEY_A));
  key(a, KEY_D, 0);
  key(a, KEY_C, 0);
  drain(a, ev, 8);
  close(a);
  close(b);
}

static void axes(void) {
  int fd = open("/dev/input/event2", O_RDONLY);
  int other = open("/dev/input/event2", O_RDONLY);
  CHECK(fd >= 0 && other >= 0);
  struct input_absinfo abs, set = {.value = 5, .minimum = 0, .maximum = 99, .resolution = 4};
  CHECK(ioctl(fd, EVIOCGABS(ABS_X), &abs) == 0 && abs.maximum == 1079 && abs.resolution == 10);
  // Every open file sees the change, value included.
  CHECK(ioctl(fd, EVIOCSABS(ABS_X), &set) == 0);
  CHECK(ioctl(other, EVIOCGABS(ABS_X), &abs) == 0 && abs.maximum == 99 && abs.value == 5 &&
        abs.resolution == 4);
  // A struct without the resolution sets none.
  CHECK(ioctl(fd, _IOC(_IOC_WRITE, 'E', 0xc0 + ABS_X, 20), &set) == 0);
  CHECK(ioctl(other, EVIOCGABS(ABS_X), &abs) == 0 && abs.resolution == 0);
  set = (struct input_absinfo){.maximum = 1079, .resolution = 10};
  CHECK(ioctl(fd, EVIOCSABS(ABS_X), &set) == 0);
  // The slots are fixed; a device without axes has none to set.
  int ts = open("/dev/input/event0", O_RDONLY);
  int kb = open("/dev/input/event1", O_RDONLY);
  CHECK(ioctl(ts, EVIOCSABS(ABS_MT_SLOT), &set) == -1 && errno == EINVAL);
  CHECK(ioctl(kb, EVIOCSABS(ABS_X), &set) == -1 && errno == EINVAL);
  // The mouse: a pointer, both wheels, the tool and the buttons.
  uint8_t bits[96];
  CHECK(ioctl(fd, EVIOCGPROP(sizeof(bits)), bits) == 8 && TEST_BIT(bits, INPUT_PROP_POINTER));
  CHECK(ioctl(fd, EVIOCGBIT(EV_REL, sizeof(bits)), bits) == 8 && TEST_BIT(bits, REL_WHEEL_HI_RES) &&
        TEST_BIT(bits, REL_HWHEEL_HI_RES));
  CHECK(ioctl(fd, EVIOCGBIT(EV_KEY, sizeof(bits)), bits) == 96 && TEST_BIT(bits, BTN_TOOL_MOUSE) &&
        TEST_BIT(bits, BTN_RIGHT) && TEST_BIT(bits, BTN_SIDE));
  close(ts);
  close(kb);
  close(fd);
  close(other);
}

static void keycodes(void) {
  int kb = open("/dev/input/event1", O_RDONLY);
  CHECK(kb >= 0);
  // Scan codes are HID usages, as hid-input's.
  unsigned int map[2] = {0x70004, 0};
  CHECK(ioctl(kb, EVIOCGKEYCODE, map) == 0 && map[1] == KEY_A);
  struct input_keymap_entry ke = {.flags = INPUT_KEYMAP_BY_INDEX, .index = 0};
  CHECK(ioctl(kb, EVIOCGKEYCODE_V2, &ke) == 0 && ke.keycode == KEY_A && ke.len == 4 &&
        ke.index == 0);
  uint32_t sc;
  memcpy(&sc, ke.scancode, 4);
  CHECK(sc == 0x70004);
  ke = (struct input_keymap_entry){.len = 3};
  CHECK(ioctl(kb, EVIOCGKEYCODE_V2, &ke) == -1 && errno == EINVAL);
  ke.len = 33;
  CHECK(ioctl(kb, EVIOCSKEYCODE_V2, &ke) == -1 && errno == EINVAL);
  map[0] = 0x12345;
  CHECK(ioctl(kb, EVIOCGKEYCODE, map) == -1 && errno == EINVAL);
  // B's usage made KEY_X: the device has no KEY_B then.
  map[0] = 0x70005;
  map[1] = KEY_MAX + 1;
  CHECK(ioctl(kb, EVIOCSKEYCODE, map) == -1 && errno == EINVAL);
  map[1] = KEY_X;
  CHECK(ioctl(kb, EVIOCSKEYCODE, map) == 0);
  uint8_t bits[96];
  CHECK(ioctl(kb, EVIOCGBIT(EV_KEY, sizeof(bits)), bits) == 96 && !TEST_BIT(bits, KEY_B) &&
        TEST_BIT(bits, KEY_X));
  ke = (struct input_keymap_entry){.len = 4, .keycode = KEY_B};
  memcpy(ke.scancode, &map[0], 4);
  CHECK(ioctl(kb, EVIOCSKEYCODE_V2, &ke) == 0);
  CHECK(ioctl(kb, EVIOCGBIT(EV_KEY, sizeof(bits)), bits) == 96 && TEST_BIT(bits, KEY_B));
  close(kb);
  // A device without a keymap.
  int ts = open("/dev/input/event0", O_RDONLY);
  CHECK(ioctl(ts, EVIOCGKEYCODE, map) == -1 && errno == EINVAL);
  close(ts);
}

static void clock_flush(void) {
  int w = open("/dev/input/event1", O_WRONLY);
  int r = open("/dev/input/event1", O_RDONLY | O_NONBLOCK);
  CHECK(w >= 0 && r >= 0);
  struct input_event ev[8];
  // Nothing queued: nothing to drop.
  int clk = CLOCK_MONOTONIC;
  CHECK(ioctl(r, EVIOCSCLOCKID, &clk) == 0);
  CHECK(read(r, ev, sizeof(ev)) == -1 && errno == EAGAIN);
  // What is queued goes, and SYN_DROPPED says so, in the new clock.
  key(w, KEY_C, 1);
  struct pollfd p = {.fd = r, .events = POLLIN};
  CHECK(poll(&p, 1, 5000) == 1);
  clk = CLOCK_REALTIME;
  int64_t before = now_ns(CLOCK_REALTIME);
  CHECK(ioctl(r, EVIOCSCLOCKID, &clk) == 0);
  CHECK(read(r, ev, sizeof(ev)) == sizeof(ev[0]) && ev[0].type == EV_SYN &&
        ev[0].code == SYN_DROPPED);
  CHECK(event_ns(&ev[0]) >= before - 1000 && event_ns(&ev[0]) <= now_ns(CLOCK_REALTIME));
  // The same clock again drops nothing.
  key(w, KEY_C, 0);
  CHECK(poll(&p, 1, 5000) == 1);
  CHECK(ioctl(r, EVIOCSCLOCKID, &clk) == 0);
  CHECK(read(r, ev, sizeof(ev)) == 2 * sizeof(ev[0]) && ev[0].code == KEY_C && ev[0].value == 0);
  close(w);
  close(r);
}

// The child of `exec`: `fd` is still the keyboard, as it was opened.
static int exec_child(int fd) {
  char name[32];
  if (ioctl(fd, EVIOCGNAME(sizeof(name)), name) < 0 || strcmp(name, "aim-keyboard")) return 1;
  struct input_event ev[8];
  // Non-blocking and writable, as opened; the clock as set.
  if (read(fd, ev, sizeof(ev)) != -1 || errno != EAGAIN) return 2;
  int64_t before = now_ns(CLOCK_MONOTONIC);
  key(fd, KEY_C, 1);
  key(fd, KEY_C, 0);
  if (drain(fd, ev, 8) != 4 || ev[0].code != KEY_C || event_ns(&ev[0]) < before - 1000 ||
      event_ns(&ev[0]) > now_ns(CLOCK_MONOTONIC))
    return 3;
  // The same open file: its grab, its node.
  if (ioctl(fd, EVIOCGRAB, 1) != 0 || ioctl(fd, EVIOCGRAB, 0) != 0) return 4;
  char link[64], target[PATH_MAX];
  snprintf(link, sizeof(link), "/proc/self/fd/%d", fd);
  ssize_t n = readlink(link, target, sizeof(target) - 1);
  if (n <= 0) return 5;
  target[n] = 0;
  if (strcmp(target, "/dev/input/event1")) return 6;
  return 0;
}

static const char* self;

static void across_exec(void) {
  int fd = open("/dev/input/event1", O_RDWR | O_NONBLOCK);
  CHECK(fd >= 0);
  int clk = CLOCK_MONOTONIC;
  CHECK(ioctl(fd, EVIOCSCLOCKID, &clk) == 0);
  char arg[16];
  snprintf(arg, sizeof(arg), "%d", fd);
  FORK_OR_SKIP(pid);
  if (pid == 0) {
    execl(self, self, "exec", arg, (char*)NULL);
    _exit(99);
  }
  int status;
  CHECK(waitpid(pid, &status, 0) == pid);
  CHECK(WIFEXITED(status) && WEXITSTATUS(status) == 0);
  close(fd);
}

static void nodes(void) {
  struct stat st;
  CHECK(stat("/dev/input/event1", &st) == 0);
  CHECK(S_ISCHR(st.st_mode) && (st.st_mode & 07777) == 0660);
  CHECK(major(st.st_rdev) == 13 && minor(st.st_rdev) == 65);
  CHECK(st.st_uid == 0 && st.st_gid == 1004);
  int fd = open("/dev/input/event1", O_RDONLY);
  struct stat fst;
  CHECK(fd >= 0 && fstat(fd, &fst) == 0 && fst.st_rdev == st.st_rdev && S_ISCHR(fst.st_mode));
  char link[64], target[PATH_MAX];
  snprintf(link, sizeof(link), "/proc/self/fd/%d", fd);
  ssize_t n = readlink(link, target, sizeof(target) - 1);
  CHECK(n > 0);
  target[n] = 0;
  CHECK(strcmp(target, "/dev/input/event1") == 0);
  close(fd);
  CHECK(open("/dev/input/event9", O_RDONLY) == -1 && errno == ENOENT);

  DIR* d = opendir("/dev/input");
  CHECK(d != NULL);
  int devices = 0;
  struct dirent* e;
  while ((e = readdir(d)) != NULL) {
    if (strncmp(e->d_name, "event", 5) == 0) {
      CHECK(e->d_type == DT_CHR);
      devices++;
    }
  }
  closedir(d);
  CHECK(devices == 3);

  // What EventHub reads to find a device's sysfs directory.
  char* real = realpath("/sys/dev/char/13:65", NULL);
  CHECK(real != NULL);
  CHECK(strcmp(real, "/sys/devices/virtual/input/input1/event1") == 0);
  free(real);
  CHECK(stat("/sys/class/input/event1", &st) == 0 && S_ISDIR(st.st_mode));
  fd = open("/sys/devices/virtual/input/input1/event1/dev", O_RDONLY);
  char buf[16] = {0};
  CHECK(fd >= 0 && read(fd, buf, sizeof(buf)) == 6 && strcmp(buf, "13:65\n") == 0);
  close(fd);
}

int main(int argc, char** argv) {
  if (argc == 3 && !strcmp(argv[1], "exec")) return exec_child(atoi(argv[2]));
  self = argv[0];
  RUN(identity);
  RUN(capabilities);
  RUN(reads);
  RUN(grab_and_revoke);
  RUN(masks);
  RUN(axes);
  RUN(keycodes);
  RUN(clock_flush);
  RUN(across_exec);
  RUN(nodes);
  DONE();
}
