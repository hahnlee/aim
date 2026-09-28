#include "../compat/darwin_media_extractor.h"
#include "../compat/darwin_vp9_decoder.h"
#include <cassert>
#include <fstream>
#include <iostream>
#include <iterator>

int main(int argc, char** argv) {
  if (argc != 2) return 2;
  std::ifstream stream(argv[1], std::ios::binary);
  std::vector<uint8_t> bytes((std::istreambuf_iterator<char>(stream)), {});
  aim::MediaExtractorState state;
  aim::ParseWebm(bytes, &state);
  assert(state.has_source && state.width > 0 && state.height > 0);
  assert(state.samples.size() >= 2 && state.samples.front().pts_us == 0);
  assert(state.samples.front().flags & 1);
  aim::Vp9Decoder decoder;
  assert(decoder.Initialize(state.width, state.height));
  std::deque<aim::DecodedVideoFrame> output;
  int64_t previous = -1;
  for (const auto& sample : state.samples) {
    assert(sample.pts_us > previous);
    assert(decoder.Decode(sample.data.data(), sample.data.size(), sample.pts_us, &output));
    previous = sample.pts_us;
  }
  assert(output.size() == state.samples.size());
  for (size_t i = 0; i < output.size(); ++i) {
    assert(output[i].width == state.width && output[i].height == state.height);
    assert(output[i].pts_us == state.samples[i].pts_us);
  }
  std::cout << "WebM+VP9 smoke PASS frames=" << output.size() << " dimensions="
            << state.width << 'x' << state.height << " last_pts=" << previous << '\n';
  aim::MediaExtractorState invalid;
  const std::vector<std::vector<uint8_t>> malformed = {
      {}, {0x00}, {0x1a}, {0x1a, 0x45}, {0x1a, 0x45, 0xdf},
      {0x1a, 0x45, 0xdf, 0xa3, 0x81, 0xff},
  };
  for (const auto& bytes : malformed) {
    aim::MediaExtractorState malformed_state;
    aim::ParseWebm(bytes, &malformed_state);
    assert(!malformed_state.has_source && malformed_state.samples.empty());
  }
  const std::vector<std::vector<uint8_t>> truncated_vints = {
      {}, {0x00}, {0x1a}, {0x1a, 0x45}, {0x1a, 0x45, 0xdf},
  };
  for (const auto& bytes : truncated_vints) {
    aim::EbmlCursor cursor{bytes.data(), bytes.size(), 0};
    uint64_t value = 0;
    size_t width = 0;
    assert(!aim::ReadEbmlVint(&cursor, &value, &width));
    assert(cursor.pos == 0);
  }
}
