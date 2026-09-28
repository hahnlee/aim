// Usage: media-decoder-smoke <VP9 IVF fixture>
// Create a non-app fixture with ffmpeg's testsrc2 + libvpx-vp9, e.g. 64x48, 10 frames.
#include "../compat/darwin_vp9_decoder.h"
#include <cassert>
#include <fstream>
#include <iostream>
#include <iterator>

static uint32_t Le32(const uint8_t* p) {
  return uint32_t(p[0]) | uint32_t(p[1]) << 8 | uint32_t(p[2]) << 16 | uint32_t(p[3]) << 24;
}

int main(int argc, char** argv) {
  if (argc != 2) return 2;
  std::ifstream stream(argv[1], std::ios::binary);
  std::vector<uint8_t> bytes((std::istreambuf_iterator<char>(stream)), {});
  assert(bytes.size() >= 32 && std::memcmp(bytes.data(), "DKIF", 4) == 0);
  assert(std::memcmp(bytes.data() + 8, "VP90", 4) == 0);
  const int width = bytes[12] | bytes[13] << 8;
  const int height = bytes[14] | bytes[15] << 8;
  const size_t frame_size = size_t(width) * height + 2 * size_t((width + 1) / 2) * ((height + 1) / 2);
  aim::Vp9Decoder decoder;
  assert(!decoder.Initialize(0, height));
  assert(decoder.Initialize(width, height));
  std::deque<aim::DecodedVideoFrame> frames;
  size_t offset = 32;
  size_t packets = 0;
  while (offset < bytes.size()) {
    assert(offset + 12 <= bytes.size());
    const size_t length = Le32(bytes.data() + offset);
    offset += 12;
    assert(length <= bytes.size() - offset);
    assert(decoder.Decode(bytes.data() + offset, length, packets * 100000, &frames));
    offset += length;
    ++packets;
  }
  assert(packets >= 2 && frames.size() == packets);
  for (size_t index = 0; index < frames.size(); ++index) {
    assert(frames[index].width == width && frames[index].height == height);
    assert(frames[index].bytes.size() == frame_size);
    assert(frames[index].pts_us == int64_t(index * 100000));
  }
  const auto first = frames.front().bytes;
  assert(first != frames.back().bytes);
  assert(decoder.Decode(nullptr, 0, 0, &frames) && frames.size() == packets);
  assert(decoder.Initialize(width, height));
  frames.clear();
  assert(decoder.Decode(bytes.data() + 44, Le32(bytes.data() + 32), 0, &frames));
  assert(frames.size() == 1 && frames.front().bytes == first);
  const uint8_t malformed[] = {0};
  assert(!decoder.Decode(malformed, sizeof(malformed), 0, &frames));
  std::cout << "VP9 smoke PASS frames=" << packets << " size=" << width << 'x' << height
            << " reset=deterministic timestamps=ordered malformed=rejected\n";
}
