#pragma once

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <optional>
#include <vector>

namespace darwin_art::socket {

enum class UnixAddressKind { Unnamed, Pathname, Abstract };

// An Android namespace name, never a Darwin filesystem path. Endpoint
// resolution and authorization must occur separately before host connect.
struct UnixAddress {
  UnixAddressKind kind;
  std::vector<uint8_t> name;
};

inline std::optional<UnixAddress> ParseUnixAddress(const void* address,
                                                  size_t length) {
  constexpr size_t kFamilySize = sizeof(uint16_t);
  constexpr size_t kAndroidPathSize = 108;
  if (address == nullptr || length < kFamilySize ||
      length > kFamilySize + kAndroidPathSize) return std::nullopt;
  uint16_t family;
  std::memcpy(&family, address, kFamilySize);
  if (family != 1) return std::nullopt;  // Android AF_UNIX
  const auto* bytes = static_cast<const uint8_t*>(address) + kFamilySize;
  const size_t size = length - kFamilySize;
  if (size == 0) return UnixAddress{UnixAddressKind::Unnamed, {}};
  if (bytes[0] == 0) {
    // Abstract names are length-delimited and may contain embedded NULs.
    return UnixAddress{UnixAddressKind::Abstract, {bytes + 1, bytes + size}};
  }
  size_t end = 0;
  while (end < size && bytes[end] != 0) ++end;
  return UnixAddress{UnixAddressKind::Pathname, {bytes, bytes + end}};
}

}  // namespace darwin_art::socket
