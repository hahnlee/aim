#include "../../compat/surfaceflinger/retained_layer_state.h"

#include "../../compat/surfaceflinger/transaction_bridge.h"

#include <array>
#include <cassert>
#include <cstdint>
#include <cstdio>

using darwin_art::surfaceflinger::CopyTransparentRegion;
using darwin_art::surfaceflinger::MergeRetainedLayer;
using darwin_art::surfaceflinger::WireLayer;

namespace {

void TestTransparentRegionCopy() {
  std::array<DarwinArtTransparentRegionRect,
             kDarwinArtMaxTransparentRegionRects>
      source{};
  source[0] = {1, 2, 11, 12};
  source[1] = {3, 4, 3, 14};  // Empty width is ignored.
  source[2] = {5, 6, 15, 16};
  source[3] = {7, 8, 17, 8};  // Empty height is ignored.
  std::array<DarwinArtTransparentRegionRect,
             kDarwinArtMaxTransparentRegionRects>
      destination{};
  const uint32_t copied = CopyTransparentRegion(
      source.data(), static_cast<uint32_t>(source.size()), destination.data());
  assert(copied == 2);
  assert(destination[0].left == 1 && destination[0].top == 2 &&
         destination[0].right == 11 && destination[0].bottom == 12);
  assert(destination[1].left == 5 && destination[1].top == 6 &&
         destination[1].right == 15 && destination[1].bottom == 16);
  assert(CopyTransparentRegion(nullptr, 1, destination.data()) == 0);
  assert(CopyTransparentRegion(source.data(), 1, nullptr) == 0);

  source[4] = {19, 20, 29, 30};
  source[5] = {31, 32, 41, 42};
  source[6] = {43, 44, 53, 54};
  source[7] = {55, 56, 65, 66};
  destination.fill({});
  assert(CopyTransparentRegion(source.data(), 99, destination.data()) == 6);
  assert(destination[5].left == 55 && destination[5].bottom == 66);
}

void TestPerChangeBitMerge() {
  WireLayer destination{};
  destination.owner_process_id = 7;
  destination.layer_id = 8;
  destination.parent_owner_process_id = 9;
  destination.parent_id = 10;
  destination.relative_parent_owner_process_id = 11;
  destination.relative_parent_id = 12;
  destination.flags = 0xaaaa;
  destination.transform = 13;
  destination.scale_x = 1.5f;
  destination.scale_y = 2.5f;
  destination.z = 14;
  destination.alpha = 0.25f;
  destination.destination_left = 10;
  destination.destination_top = 20;
  destination.destination_right = 110;
  destination.destination_bottom = 220;
  destination.position_x = 15;
  destination.position_y = 16;

  WireLayer source{};
  source.owner_process_id = 101;
  source.layer_id = 102;
  source.what = DARWIN_ART_SF_REPARENT;
  source.parent_owner_process_id = 103;
  source.parent_id = 104;
  source.relative_parent_owner_process_id = 105;
  source.relative_parent_id = 106;
  MergeRetainedLayer(destination, source);
  assert(destination.owner_process_id == 101 && destination.layer_id == 102);
  assert(destination.parent_owner_process_id == 103 &&
         destination.parent_id == 104);
  assert(destination.relative_parent_owner_process_id == 11 &&
         destination.relative_parent_id == 12);

  source = {};
  source.owner_process_id = destination.owner_process_id;
  source.layer_id = destination.layer_id;
  source.what = DARWIN_ART_SF_REPARENT;
  MergeRetainedLayer(destination, source);
  assert(destination.parent_owner_process_id == 0 &&
         destination.parent_id == 0);

  destination.relative_parent_owner_process_id = 201;
  destination.relative_parent_id = 202;
  source = {};
  source.owner_process_id = destination.owner_process_id;
  source.layer_id = destination.layer_id;
  source.what = DARWIN_ART_SF_RELATIVE_LAYER_CHANGED;
  source.relative_parent_owner_process_id = 203;
  source.relative_parent_id = 204;
  MergeRetainedLayer(destination, source);
  assert(destination.relative_parent_owner_process_id == 203 &&
         destination.relative_parent_id == 204);
  assert(destination.parent_id == 0);

  source = {};
  source.owner_process_id = destination.owner_process_id;
  source.layer_id = destination.layer_id;
  source.what = DARWIN_ART_SF_LAYER_CHANGED;
  source.z = 205;
  MergeRetainedLayer(destination, source);
  assert(destination.z == 205);
  assert(destination.relative_parent_owner_process_id == 0 &&
         destination.relative_parent_id == 0);

  destination.flags = 0xaaaa;
  source = {};
  source.owner_process_id = destination.owner_process_id;
  source.layer_id = destination.layer_id;
  source.what = DARWIN_ART_SF_FLAGS_CHANGED;
  source.flags = 0x0005;
  source.mask = 0x000f;
  MergeRetainedLayer(destination, source);
  assert(destination.flags == 0xaaa5u);
  source.flags = 0xf0f0;
  source.mask = 0x00ff;
  MergeRetainedLayer(destination, source);
  assert(destination.flags == 0xaaf0u);

  source = {};
  source.owner_process_id = destination.owner_process_id;
  source.layer_id = destination.layer_id;
  source.what = DARWIN_ART_SF_BUFFER_TRANSFORM_CHANGED;
  source.transform = 206;
  MergeRetainedLayer(destination, source);
  assert(destination.transform == 206);

  source.what = DARWIN_ART_SF_MATRIX_CHANGED;
  source.scale_x = 3.5f;
  source.scale_y = 4.5f;
  MergeRetainedLayer(destination, source);
  assert(destination.scale_x == 3.5f && destination.scale_y == 4.5f);

  source.what = DARWIN_ART_SF_ALPHA_CHANGED;
  source.alpha = 0.75f;
  MergeRetainedLayer(destination, source);
  assert(destination.alpha == 0.75f);
}

void TestBufferGeometryAndNullBuffer() {
  WireLayer destination{};
  destination.destination_left = 30;
  destination.destination_top = 40;
  destination.destination_right = 30;
  destination.destination_bottom = 40;
  WireLayer source{};
  source.what = DARWIN_ART_SF_BUFFER_CHANGED;
  source.iosurface_id = 0;  // A genuine buffer-removal occurrence.
  source.width = 640;
  source.height = 480;
  source.producer_bottom_left = 1;
  source.source_left = 1;
  source.source_top = 2;
  source.source_right = 639;
  source.source_bottom = 479;
  MergeRetainedLayer(destination, source);
  assert(destination.iosurface_id == 0);
  assert(destination.width == 640 && destination.height == 480);
  assert(destination.destination_left == 30 &&
         destination.destination_top == 40 &&
         destination.destination_right == 670 &&
         destination.destination_bottom == 520);
  assert(destination.source_left == 1 && destination.source_bottom == 479);

  destination.destination_left = 10;
  destination.destination_top = 20;
  destination.destination_right = 110;
  destination.destination_bottom = 220;
  source = {};
  source.what = DARWIN_ART_SF_POSITION_CHANGED;
  source.destination_left = 50;
  source.destination_top = 60;
  source.position_x = 51;
  source.position_y = 61;
  MergeRetainedLayer(destination, source);
  assert(destination.destination_left == 50 && destination.destination_top == 60 &&
         destination.destination_right == 150 &&
         destination.destination_bottom == 260);
  assert(destination.position_x == 51 && destination.position_y == 61);

  source = {};
  source.what = DARWIN_ART_SF_CROP_CHANGED;
  source.has_crop = 1;
  source.crop_left = 1;
  source.crop_top = 2;
  source.crop_right = 3;
  source.crop_bottom = 4;
  MergeRetainedLayer(destination, source);
  assert(destination.has_crop == 1 && destination.crop_right == 3);

  source = {};
  source.what = DARWIN_ART_SF_DESTINATION_FRAME_CHANGED;
  source.destination_left = 70;
  source.destination_top = 80;
  source.destination_right = 170;
  source.destination_bottom = 280;
  MergeRetainedLayer(destination, source);
  assert(destination.destination_left == 70 &&
         destination.destination_bottom == 280);
}

void TestTransparentMerge() {
  WireLayer destination{};
  destination.transparent_region_count = 1;
  destination.transparent_region[0] = {90, 90, 91, 91};
  WireLayer source{};
  source.what = DARWIN_ART_SF_TRANSPARENT_REGION_CHANGED;
  source.transparent_region_count = 3;
  source.transparent_region[0] = {1, 2, 11, 12};
  source.transparent_region[1] = {4, 5, 4, 6};
  source.transparent_region[2] = {13, 14, 23, 24};
  MergeRetainedLayer(destination, source);
  assert(destination.transparent_region_count == 2);
  assert(destination.transparent_region[0].left == 1);
  assert(destination.transparent_region[1].right == 23);
}

}  // namespace

int main() {
  TestTransparentRegionCopy();
  TestPerChangeBitMerge();
  TestBufferGeometryAndNullBuffer();
  TestTransparentMerge();
  std::puts("retained-layer-state: PASS");
}
