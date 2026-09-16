#include "../../compat/surfaceflinger/display_attachment.h"

#include <cassert>
#include <cstdio>
#include <set>
#include <unordered_map>

using darwin_art::surfaceflinger::IsAttachedToDisplayAnchor;

int main() {
  std::unordered_map<uint32_t, uint32_t> parents{
      {1, 0}, {2, 1}, {3, 2}, {7, 0}, {8, 7}};
  const std::set<uint32_t> display_one{1};
  const std::set<uint32_t> display_two{7};

  assert(IsAttachedToDisplayAnchor(1, parents, display_one));
  assert(IsAttachedToDisplayAnchor(2, parents, display_one));
  assert(IsAttachedToDisplayAnchor(3, parents, display_one));
  assert(!IsAttachedToDisplayAnchor(8, parents, display_one));
  assert(IsAttachedToDisplayAnchor(8, parents, display_two));
  assert(!IsAttachedToDisplayAnchor(9, parents, display_one));

  parents[2] = 7;
  assert(!IsAttachedToDisplayAnchor(3, parents, display_one));
  assert(IsAttachedToDisplayAnchor(3, parents, display_two));

  parents[7] = 8;
  assert(!IsAttachedToDisplayAnchor(8, parents, display_two));

  std::puts("surfaceflinger-display-attachment: PASS");
}
