#pragma once

#include "service_darwin.h"

#include <array>
#include <cstdint>
#include <memory>
#include <type_traits>
#include <vector>

namespace darwin_art::surfaceflinger {

inline constexpr std::array<char, 8> kRequestMagic{'D', 'A', 'R', 'T', 'S', 'F', '0', '7'};
inline constexpr std::array<char, 8> kResponseMagic{'D', 'A', 'R', 'T', 'S', 'F', 'R', '7'};
inline constexpr uint32_t kProtocolVersion = 11;
inline constexpr uint32_t kMaximumLayers = 4096;

enum class RequestKind : uint32_t {
  kStructuralCommit = 0,
  kDisplayPresent = 1,
  kLayerTransaction = 2,
};

struct RequestHeader {
  char magic[8];
  uint32_t version;
  uint32_t kind;
  uint32_t process_id;
  uint32_t target_iosurface_id;
  uint32_t target_width;
  uint32_t target_height;
  uint32_t layer_count;
  uint32_t has_producer_fence;
  uint64_t transaction_id;
};

struct WireLayer {
  uint32_t owner_process_id;
  uint32_t layer_id;
  uint32_t parent_owner_process_id;
  uint32_t parent_id;
  uint32_t relative_parent_owner_process_id;
  uint32_t relative_parent_id;
  uint32_t iosurface_id;
  uint32_t width;
  uint32_t height;
  uint64_t what;
  uint32_t flags;
  uint32_t mask;
  uint32_t transform;
  uint32_t producer_bottom_left;
  int32_t source_left;
  int32_t source_top;
  int32_t source_right;
  int32_t source_bottom;
  int32_t destination_left;
  int32_t destination_top;
  int32_t destination_right;
  int32_t destination_bottom;
  int32_t position_x;
  int32_t position_y;
  float scale_x;
  float scale_y;
  uint32_t has_crop;
  int32_t crop_left;
  int32_t crop_top;
  int32_t crop_right;
  int32_t crop_bottom;
  int32_t z;
  float alpha;
  uint32_t transparent_region_count;
  DarwinArtTransparentRegionRect transparent_region[
      kDarwinArtMaxTransparentRegionRects];
};

enum class CommitDisposition : uint32_t {
  Unknown = 0,
  RejectedBeforeCommit = 1,
  Committed = 2,
};

struct ResponseHeader {
  char magic[8];
  uint32_t version;
  int32_t status;
  CommitDisposition commit;
  uint32_t has_completion_fence;
};

struct CompositionJob {
  RequestHeader header{};
  std::vector<WireLayer> incoming;
  int producer_descriptor = -1;
  int completion_descriptor = -1;
  bool fence_failed = false;
  // Native storage is acquired before the server response and survives queue
  // delay/fence waits. This is not part of the wire ABI.
  std::shared_ptr<const class ImportedCompositionBackings> backings;
  // Output identity is not a wire handle. Admission retains the exact epoch;
  // its backing survives retirement, but retirement forbids publication.
  std::shared_ptr<const class OutputEpoch> output_epoch;
  std::shared_ptr<class TransactionReply> reply;
};

static_assert(std::is_trivially_copyable_v<RequestHeader>);
static_assert(std::is_trivially_copyable_v<WireLayer>);
static_assert(std::is_trivially_copyable_v<ResponseHeader>);

}  // namespace darwin_art::surfaceflinger
