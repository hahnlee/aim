// fs-verity digests against the kernel's algorithm (SHA-256, 4 KiB blocks, no
// salt): the empty file's digest is fs-verity's well-known value, and the
// others cover one block, a partial second block and a two-level tree.
#include "../../compat/security/verity_utils_jni.h"

#include <cassert>
#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

namespace {
std::string Hex(const uint8_t* digest) {
  static const char kDigits[] = "0123456789abcdef";
  std::string text;
  for (int i = 0; i < 32; ++i) {
    text += kDigits[digest[i] >> 4];
    text += kDigits[digest[i] & 15];
  }
  return text;
}

std::string Digest(const std::vector<uint8_t>& content) {
  uint8_t digest[32];
  darwin_art::security::ComputeFsverityDigest(content.data(), content.size(), digest);
  return Hex(digest);
}
}  // namespace

// Test-only: the JNI registration is not exercised here.
namespace darwin_art::security {}

int main() {
  assert(Digest({}) == "3d248ca542a24fc62d1c43b916eae5016878e2533c88238480b26128a1f1af95");
  const std::string hello = "hello\n";
  assert(Digest(std::vector<uint8_t>(hello.begin(), hello.end())) ==
         "9c76eecc7b76fcb46199cb27b90cf59a660e10575bb0412128905129d5b1c2aa");
  assert(Digest(std::vector<uint8_t>(5000, 'x')) ==
         "838b2c37e0ad28bb545e3edbe665a04a8ef1b3d559fe568db29b546e4458e1d4");
  std::vector<uint8_t> big;
  for (int repeat = 0; repeat < 4096; ++repeat) {
    for (int value = 0; value < 256; ++value) big.push_back(static_cast<uint8_t>(value));
  }
  // 1 MiB = 256 data blocks: 8 KiB of hashes, a second Merkle level.
  assert(Digest(big) == "95fd9c86efec24f88bb1ae3793095b1728f1ae623845a931037b7c90ca49f394");
  std::puts("verity-utils: fs-verity digests (empty, one block, two blocks, two levels) PASS");
}
