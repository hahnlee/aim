#include <array>
#include <cstdint>
#include <iostream>

#include "base/memory_region.h"
#include "base/time_utils.h"

int main() {
  std::array<uint8_t, 4> source = {1, 2, 3, 4};
  std::array<uint8_t, 4> destination = {};
  art::MemoryRegion source_region(source.data(), source.size());
  art::MemoryRegion destination_region(destination.data(), destination.size());
  destination_region.CopyFrom(0, source_region);

  if (destination != source) {
    return 1;
  }

  std::cout << "libartbase Darwin: " << art::PrettyDuration(1'500'000) << '\n';
  return 0;
}
