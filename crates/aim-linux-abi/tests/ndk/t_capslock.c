// AppKit Caps Lock translation through Linux evdev: source-switch events
// leave the lock alone; real lock transitions produce EV_KEY down/up.
// Each key transition ends in SYN_REPORT (Documentation/input/event-codes.rst).
#include <fcntl.h>
#include <linux/input.h>
#include <stdint.h>
#include <sys/ioctl.h>
#include <unistd.h>

#include "check.h"

static void capslock(void) {
  int fd = open("/dev/input/event1", O_RDONLY | O_NONBLOCK);
  CHECK(fd >= 0);
  for (uint8_t step = 0; step < 7; step++) {
    CHECK(write(3, &step, 1) == 1);
    uint8_t ack;
    CHECK(read(3, &ack, 1) == 1 && ack == step);
    // Only steps 2 and 5 change the actual lock flag. The other Caps Lock
    // events include repeats, source switches and a return after focus loss.
    int changed = step == 2 || step == 5;
    const unsigned short codes[] = {KEY_CAPSLOCK, KEY_CAPSLOCK, KEY_A, KEY_A};
    for (int i = changed ? 0 : 2; i < 4; i++) {
      struct input_event events[2];
      CHECK(read(fd, events, sizeof(events)) == sizeof(events));
      CHECK(events[0].type == EV_KEY && events[0].code == codes[i]);
      CHECK(events[0].value == (i % 2 == 0));
      CHECK(events[1].type == EV_SYN && events[1].code == SYN_REPORT);
    }
    struct input_event event;
    CHECK(read(fd, &event, sizeof(event)) == -1 && errno == EAGAIN);
    uint8_t held[(KEY_MAX + 8) / 8] = {0};
    CHECK(ioctl(fd, EVIOCGKEY(sizeof(held)), held) > 0);
    CHECK(!(held[KEY_CAPSLOCK / 8] & (1 << (KEY_CAPSLOCK % 8))));
  }
  close(fd);
}

int main(void) {
  RUN(capslock);
  DONE();
}
