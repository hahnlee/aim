#include <mach/mach_vm.h>
#include <sys/mman.h>
#include <unistd.h>

#include <cstdint>
#include <cstdio>
#include <cstring>

int main(int argc, char** argv) {
  const size_t page = static_cast<size_t>(getpagesize());
  const size_t metadata_size = 32u * 1024u * 1024u;
  const bool fragmented = argc == 2 && std::strcmp(argv[1], "--fragmented") == 0;
  // Keep the range immediately above metadata occupied: MAP_JIT must be
  // allowed to choose another address, as in ART's metadata-first fallback.
  const size_t reservation_size = metadata_size * (fragmented ? 2u : 1u);
  void* metadata = mmap(nullptr, reservation_size, PROT_READ | PROT_WRITE,
                        MAP_PRIVATE | MAP_ANON, -1, 0);
  if (metadata == MAP_FAILED) {
    perror("metadata mmap");
    return 1;
  }
  const uintptr_t metadata_begin = reinterpret_cast<uintptr_t>(metadata);
  const uintptr_t hint = metadata_begin + metadata_size;
  void* code = mmap(nullptr, metadata_size,
                    PROT_READ | PROT_WRITE | PROT_EXEC,
                    MAP_PRIVATE | MAP_ANON | MAP_JIT, -1, 0);
  if (code == MAP_FAILED) {
    perror("MAP_JIT kernel placement");
    munmap(metadata, reservation_size);
    return 2;
  }
  const uintptr_t code_begin = reinterpret_cast<uintptr_t>(code);
  const uintptr_t code_end = code_begin + metadata_size;
  const bool ordered = metadata_begin + metadata_size <= code_begin;
  const bool span_ok = code_end >= metadata_begin &&
                       code_end - metadata_begin <= UINT32_MAX;
  const bool relocation_ok = !fragmented || code_begin != hint;
  std::printf("JIT layout smoke: mode=%s metadata=%p code=%p adjacent=%p page=%zu ordered=%s span=%s relocation=%s\n",
              fragmented ? "fragmented" : "ordinary", metadata, code,
              reinterpret_cast<void*>(hint), page,
              ordered ? "pass" : "fail", span_ok ? "pass" : "fail",
              relocation_ok ? "pass" : "fail");
  munmap(code, metadata_size);
  munmap(metadata, reservation_size);
  return ordered && span_ok && relocation_ok ? 0 : 3;
}
