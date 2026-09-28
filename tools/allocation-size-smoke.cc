#include "asm_defines.h"
#include <cassert>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <initializer_list>
#include <sys/mman.h>
#include <unistd.h>

extern "C" uint64_t darwin_test_array_size(void* klass, uint32_t count);

int main() {
  const size_t page = static_cast<size_t>(getpagesize());
  const uintptr_t address = AIM_REFERENCE_BASE + 0x30000000ULL;
  // A hint, never MAP_FIXED: do not replace an existing mapping.
  void* mapping = mmap(reinterpret_cast<void*>(address), page * 2,
                       PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0);
  assert(mapping != MAP_FAILED);
  assert(reinterpret_cast<uintptr_t>(mapping) == address);
  auto* klass = static_cast<uint8_t*>(mapping);
  auto* component = klass + page;
  uint32_t reference = static_cast<uint32_t>(reinterpret_cast<uintptr_t>(component));
#ifdef USE_HEAP_POISONING
  reference = -reference;
#endif
  std::memcpy(klass + MIRROR_CLASS_COMPONENT_TYPE_OFFSET, &reference, sizeof(reference));
  for (uint32_t shift = 0; shift != 4; ++shift) {
    uint32_t type = shift << PRIMITIVE_TYPE_SIZE_SHIFT_SHIFT;
    std::memcpy(component + MIRROR_CLASS_OBJECT_PRIMITIVE_TYPE_OFFSET, &type, sizeof(type));
    for (uint32_t count : {0u, 1u, 7u, 8193u, 0x7fffffffu, 0x80000000u, 0xffffffffu}) {
      const uint64_t header = shift == 3 ? MIRROR_LONG_ARRAY_DATA_OFFSET
                                         : MIRROR_INT_ARRAY_DATA_OFFSET;
      const uint64_t expected = (header + (uint64_t{count} << shift) + OBJECT_ALIGNMENT_MASK)
                                & ~uint64_t{OBJECT_ALIGNMENT_MASK};
      assert(darwin_test_array_size(klass, count) == expected);
    }
  }
  assert(munmap(mapping, page * 2) == 0);
  puts("TLAB size macro PASS: high-address component, 4 widths, 7 counts, aligned uint64 size");
}
