#include "loader/guest_open_request.h"
#include <cassert>
#include <cstring>
#include <initializer_list>
int main() {
  using darwin_art::loader::GuestImageUnloadable;
  using darwin_art::loader::GuestOpenRequest;
  using darwin_art::loader::ParseGuestOpenRequest;
  DarwinArtAndroidDlExtInfo info{};
  GuestOpenRequest request;
  // bionic accepts every LAZY/NOW/LOCAL combination; binding is always eager.
  for (int flags : {0, 1, 2, 3}) {
    assert(!ParseGuestOpenRequest(flags, nullptr, &request));
    assert(!request.global && !request.nodelete);
  }
  assert(!ParseGuestOpenRequest(2, &info, &request));
  // Unity: dlopen("libmediandk.so", RTLD_NOW | RTLD_GLOBAL).
  assert(!ParseGuestOpenRequest(0x102, nullptr, &request));
  assert(request.global && !request.nodelete);
  assert(!ParseGuestOpenRequest(0x1002, nullptr, &request));
  assert(!request.global && request.nodelete);
  assert(!ParseGuestOpenRequest(0x1101, &info, &request));
  assert(request.global && request.nodelete);
  // A rejected request never leaks modifiers from an earlier parse.
  assert(ParseGuestOpenRequest(0x106, nullptr, &request));
  assert(!request.global && !request.nodelete);
  // RTLD_NOLOAD is rejected explicitly, alone or combined.
  for (int flags : {4, 6, 0x104, 0x1004}) {
    const char* error = ParseGuestOpenRequest(flags, nullptr, &request);
    assert(error && std::strstr(error, "RTLD_NOLOAD"));
  }
  // Unknown bits (including Darwin RTLD_GLOBAL=0x8/RTLD_LOCAL=0x4 confusion
  // beyond NOLOAD) remain invalid bionic flags.
  for (int flags : {0x8, 0x10, 0x200, 0x102 | 0x8, 0x2000, -1}) {
    const char* error = ParseGuestOpenRequest(flags, nullptr, &request);
    assert(error && std::strstr(error, "invalid Android dlopen flags"));
  }
  for (uint64_t flags : {uint64_t{1}, uint64_t{0x10}, uint64_t{0x200}, UINT64_MAX}) {
    info.flags = flags;
    assert(ParseGuestOpenRequest(2, &info, &request));
    assert(ParseGuestOpenRequest(0x102, &info, &request));
  }
  assert(ParseGuestOpenRequest(2, nullptr, nullptr));
  // bionic soinfo::can_unload.
  assert(GuestImageUnloadable(false, false));
  assert(!GuestImageUnloadable(true, false));
  assert(!GuestImageUnloadable(false, true));
  assert(!GuestImageUnloadable(true, true));
}
