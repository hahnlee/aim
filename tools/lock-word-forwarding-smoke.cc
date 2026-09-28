// Focused regression for the Darwin compressed-reference forwarding payload.
// This is compiled against the staged, patched ART runtime headers by the
// runtime bootstrap smoke harness.
#include "lock_word-inl.h"

#include <cstdint>

int main() {
  constexpr uintptr_t base = 0x0000010000000000ULL;
  constexpr uintptr_t limit = base + 4ULL * 1024ULL * 1024ULL * 1024ULL;
  constexpr uintptr_t targets[] = {
      base + 64ULL * 1024ULL,
      base + 0x12345678ULL,
      base + 0xfffffff8ULL,
  };
  for (const uintptr_t target : targets) {
    if (target < base || target >= limit) return 1;
    const art::LockWord word = art::LockWord::FromForwardingAddress(target);
    if (word.GetState() != art::LockWord::kForwardingAddress) return 2;
    if (word.ForwardingAddress() != target) return 3;
  }
  return 0;
}
