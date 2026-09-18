#pragma once

#include <array>
#include <cstdint>
#include <type_traits>

namespace darwin_art::surfaceflinger {

inline constexpr std::array<char, 8> kOutputMagic{'D','A','R','T','O','U','T','1'};
inline constexpr uint32_t kOutputProtocolVersion = 1;
enum class OutputOperation : uint32_t { Register = 1, Replace = 2, Retire = 3 };

struct OutputToken {
  uint64_t server_instance = 0;
  uint64_t serial = 0;
  bool operator==(const OutputToken&) const = default;
};

// Fixed-size native control frame. The owning connection is the authority;
// the token/generation identify stale requests, not permission to mutate an
// output over any unrelated connection. No layer or fence travels here.
struct OutputRequest {
  std::array<char, 8> magic = kOutputMagic;
  uint32_t version = kOutputProtocolVersion;
  OutputOperation operation = OutputOperation::Register;
  OutputToken token{};
  uint64_t generation = 0;
  uint32_t iosurface_id = 0;
  uint32_t physical_width = 0;
  uint32_t physical_height = 0;
  uint32_t logical_width = 0;
  uint32_t logical_height = 0;
  uint32_t reserved = 0;
};

struct OutputResponse {
  std::array<char, 8> magic = kOutputMagic;
  uint32_t version = kOutputProtocolVersion;
  int32_t status = 0;
  OutputToken token{};
  uint64_t generation = 0;
};

static_assert(sizeof(OutputRequest) == 64);
static_assert(sizeof(OutputResponse) == 40);
static_assert(std::is_trivially_copyable_v<OutputRequest>);
static_assert(std::is_trivially_copyable_v<OutputResponse>);

}  // namespace darwin_art::surfaceflinger
