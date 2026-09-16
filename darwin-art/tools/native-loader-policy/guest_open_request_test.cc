#include "loader/guest_open_request.h"
#include <cassert>
#include <initializer_list>
int main() {
  using darwin_art::loader::LegacyOpenRequestError;
  DarwinArtAndroidDlExtInfo info{};
  assert(!LegacyOpenRequestError(1, nullptr));
  assert(!LegacyOpenRequestError(2, &info));
  for (auto flags : {0, 3, 4, 0x102, 0x1002, -1})
    assert(LegacyOpenRequestError(flags, nullptr));
  for (uint64_t flags : {uint64_t{1}, uint64_t{0x10}, uint64_t{0x200}, UINT64_MAX}) {
    info.flags = flags;
    assert(LegacyOpenRequestError(2, &info));
  }
}
