// The original EventHub (the image's libinputreader.so, what inputflinger's
// InputReader reads through) on the display server's evdev devices: it
// scans /dev/input, opens and classifies the devices, loads their
// configuration and key layout, and reads a key a writer injects.
//
// EventHub is called through its exported symbols; RawEvent and the
// returned vector have the platform's layout (EventHub.h, libc++).
#include <dlfcn.h>
#include <fcntl.h>
#include <linux/input.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include <string>
#include <vector>

struct RawEvent {
  int64_t when;
  int64_t readTime;
  int32_t deviceId;
  int32_t type;
  int32_t code;
  int32_t value;
};

enum { DEVICE_ADDED = 0x10000000 };

using Ctor = void (*)(void*);
using GetEvents = std::vector<RawEvent> (*)(void*, int);
using Dump = void (*)(const void*, std::string&);

int main() {
  // Without a generated linker configuration the default namespace does not
  // search the APEXes; load libinputreader's APEX dependencies by path.
  for (const char* lib : {"/apex/com.android.os.statsd/lib64/libstatssocket.so",
                          "/apex/com.android.os.statsd/lib64/libstatspull.so"}) {
    if (!dlopen(lib, RTLD_NOW | RTLD_GLOBAL)) printf("note: %s\n", dlerror());
  }
  void* lib = dlopen("libinputreader.so", RTLD_NOW);
  if (!lib) {
    printf("FAIL dlopen: %s\n", dlerror());
    return 1;
  }
  auto ctor = reinterpret_cast<Ctor>(dlsym(lib, "_ZN7android8EventHubC1Ev"));
  auto get = reinterpret_cast<GetEvents>(dlsym(lib, "_ZN7android8EventHub9getEventsEi"));
  auto dump = reinterpret_cast<Dump>(dlsym(
      lib,
      "_ZNK7android8EventHub4dumpERNSt3__112basic_stringIcNS1_11char_traitsIcEENS1_"
      "9allocatorIcEEEE"));
  if (!ctor || !get || !dump) {
    printf("FAIL dlsym: %s\n", dlerror());
    return 1;
  }
  // Larger than EventHub; it lives for the program.
  void* hub = calloc(1, 1 << 16);
  ctor(hub);

  // The first call scans /dev/input: the three devices and EventHub's own
  // virtual keyboard.
  int added = 0;
  for (int i = 0; i < 20 && added < 4; i++) {
    for (const RawEvent& e : get(hub, 1000)) {
      if (e.type == DEVICE_ADDED) {
        printf("added device %d\n", e.deviceId);
        added++;
      }
    }
  }
  std::string text;
  dump(hub, text);
  printf("%s\n", text.c_str());
  if (added != 4) {
    printf("FAIL: %d devices added\n", added);
    return 1;
  }

  // A key, written into the keyboard as a tool would.
  int kb = open("/dev/input/event1", O_WRONLY);
  struct input_event in[2];
  memset(in, 0, sizeof(in));
  in[0].type = EV_KEY;
  in[0].code = KEY_B;
  in[0].value = 1;
  in[1].type = EV_SYN;
  in[1].code = SYN_REPORT;
  if (kb < 0 || write(kb, in, sizeof(in)) != sizeof(in)) {
    printf("FAIL: write the keyboard\n");
    return 1;
  }
  bool key = false;
  for (int i = 0; i < 20 && !key; i++) {
    for (const RawEvent& e : get(hub, 1000)) {
      if (e.type == EV_KEY) {
        printf("key: device %d code %d value %d\n", e.deviceId, e.code, e.value);
        key |= e.code == KEY_B && e.value == 1;
      }
    }
  }
  printf(key ? "PASS\n" : "FAIL: no key\n");
  return key ? 0 : 1;
}
