// Isolated descriptor-boundary test. The table below substitutes only the Rust
// FFI; backing memory, dup, mmap and access enforcement use the Darwin kernel.
#include "../compat/memory/application_descriptor.h"
#include "../compat/memory/application_memory.h"
#include <cassert>
#include <cerrno>
#include <cstdint>
#include <cstdio>
#include <map>
#include <sys/mman.h>
#include <unistd.h>

#include "tests/application-descriptor-table.h"
int main() {
  using namespace darwin_art::memory;
  // Creation releases its temporary native descriptor. A second creation can
  // reuse that number before either mapping; inode ownership must not collide.
  int first = CreateApplicationDescriptor(4096);
  int second = CreateApplicationDescriptor(4096);
  assert(first >= 0 && second >= 0);
  void* first_map = MapApplicationDescriptor(first, 4096, true);
  void* second_map = MapApplicationDescriptor(second, 4096, true);
  assert(first_map != MAP_FAILED && second_map != MAP_FAILED);
  int first_reader = DuplicateApplicationDescriptorReader(first);
  int second_reader = DuplicateApplicationDescriptorReader(second);
  assert(first_reader >= 0 && second_reader >= 0);
  for (int fd : {first, second, first_reader, second_reader})
    assert(CloseApplicationDescriptor(fd) == 0);
  assert(UnmapApplicationMemory(first_map, 4096) == 0);
  assert(UnmapApplicationMemory(second_map, 4096) == 0);
  for (int i = 0; i < 100; ++i) {
    int writer = CreateApplicationDescriptor(4096);
    assert(writer >= 100000);
    auto* writable = static_cast<int*>(MapApplicationDescriptor(writer, 4096, true));
    assert(writable != MAP_FAILED);
    *writable = i + 42;
    int reader = DuplicateApplicationDescriptorReader(writer);
    assert(reader >= 100000 && reader != writer);
    auto* readable = static_cast<int*>(MapApplicationDescriptor(reader, 4096, false));
    assert(readable != MAP_FAILED && *readable == i + 42);
    assert(MapApplicationDescriptor(reader, 4096, true) == MAP_FAILED);
    // A live native FD number must never be accepted as a guest descriptor.
    assert(MapApplicationDescriptor(table.at(reader), 4096, false) == MAP_FAILED);
    assert(CloseApplicationDescriptor(writer) == 0);
    assert(CloseApplicationDescriptor(reader) == 0);
    assert(*readable == i + 42);
    assert(UnmapApplicationMemory(writable, 4096) == 0);
    assert(UnmapApplicationMemory(readable, 4096) == 0);
    assert(table.empty());
  }
  puts("application descriptor boundary: guest namespace, read-only mapping, close-before-unmap PASS (isolated FFI table)");
}
