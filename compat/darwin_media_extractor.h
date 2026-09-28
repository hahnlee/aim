#pragma once

// Shared, bounded WebM/Matroska state used by both the Java and NDK media
// extractor facades.  The parser intentionally handles the subset needed by
// the Android startup movies (VP9 SimpleBlocks) without depending on a host
// media process.

#include <cstddef>
#include <cstdint>
#include <string>
#include <utility>
#include <vector>

namespace aim {

constexpr size_t kMediaExtractorMaxBytes = 64u * 1024u * 1024u;

struct MediaExtractorState {
  bool has_source = false;
  int64_t duration_us = 0;
  int width = 0;
  int height = 0;
  int64_t timecode_scale = 1000000;
  uint64_t video_track_number = 1;
  bool have_video_track = false;
  int selected_track = -1;
  size_t sample_index = 0;

  struct Sample {
    std::vector<uint8_t> data;
    int64_t pts_us = 0;
    uint32_t flags = 0;
  };
  std::vector<Sample> samples;
};

struct EbmlCursor {
  const uint8_t* data = nullptr;
  size_t size = 0;
  size_t pos = 0;
};

inline bool ReadEbmlVint(EbmlCursor* cursor, uint64_t* value, size_t* width,
                         bool keep_marker = false) {
  if (cursor == nullptr || value == nullptr || width == nullptr ||
      cursor->data == nullptr || cursor->pos >= cursor->size) {
    return false;
  }
  const uint8_t first = cursor->data[cursor->pos];
  uint8_t mask = 0x80;
  size_t count = 1;
  while (count <= 8 && (first & mask) == 0) {
    mask >>= 1;
    ++count;
  }
  // Check the remaining span before subtracting.  The old form
  // `pos > size - count` wrapped when a truncated VINT was longer than the
  // input and allowed the loop below to read past the buffer.
  if (count > 8 || cursor->pos > cursor->size ||
      count > cursor->size - cursor->pos) {
    return false;
  }
  uint64_t result = keep_marker
                        ? static_cast<uint64_t>(first)
                        : static_cast<uint64_t>(first & (mask - 1));
  for (size_t i = 1; i < count; ++i) {
    result = (result << 8) | cursor->data[cursor->pos + i];
  }
  cursor->pos += count;
  *value = result;
  *width = count;
  return true;
}

inline bool ReadEbmlElement(EbmlCursor* cursor, uint64_t* id,
                            uint64_t* length, size_t* payload) {
  size_t id_width = 0;
  size_t size_width = 0;
  if (!ReadEbmlVint(cursor, id, &id_width, true) ||
      !ReadEbmlVint(cursor, length, &size_width, false)) {
    return false;
  }
  *payload = cursor->pos;
  // An all-ones EBML size denotes an element extending to the parent end.
  const uint64_t unknown = (uint64_t{1} << (7 * size_width)) - 1;
  if (*length == unknown) *length = cursor->size - cursor->pos;
  return *length <= cursor->size - cursor->pos;
}

inline uint64_t ReadUnsigned(const uint8_t* data, size_t length) {
  if (data == nullptr || length > sizeof(uint64_t)) return 0;
  uint64_t value = 0;
  for (size_t i = 0; i < length; ++i) value = (value << 8) | data[i];
  return value;
}

inline bool ReadSigned16(const uint8_t* data, size_t length, int64_t* value) {
  if (data == nullptr || length != 2 || value == nullptr) return false;
  *value = static_cast<int16_t>((static_cast<uint16_t>(data[0]) << 8) | data[1]);
  return true;
}

inline void ParseWebmTrack(const uint8_t* data, size_t length,
                           MediaExtractorState* state,
                           uint64_t* track_number, bool* is_video) {
  if (data == nullptr || state == nullptr || track_number == nullptr ||
      is_video == nullptr) {
    return;
  }
  EbmlCursor cursor{data, length, 0};
  while (cursor.pos < cursor.size) {
    uint64_t id = 0;
    uint64_t element_length = 0;
    size_t payload = 0;
    if (!ReadEbmlElement(&cursor, &id, &element_length, &payload)) break;
    const uint8_t* bytes = cursor.data + payload;
    if (id == 0xD7 && element_length <= sizeof(uint64_t)) {
      *track_number = ReadUnsigned(bytes, static_cast<size_t>(element_length));
    } else if (id == 0x83 && element_length <= sizeof(uint64_t)) {
      *is_video = ReadUnsigned(bytes, static_cast<size_t>(element_length)) == 1;
    } else if (id == 0x86 && element_length >= 3 && element_length <= 64) {
      const std::string codec(reinterpret_cast<const char*>(bytes),
                              static_cast<size_t>(element_length));
      if (codec == "V_VP9") *is_video = true;
    } else if (id == 0xE0) {
      EbmlCursor video{bytes, static_cast<size_t>(element_length), 0};
      while (video.pos < video.size) {
        uint64_t video_id = 0;
        uint64_t video_length = 0;
        size_t video_payload = 0;
        if (!ReadEbmlElement(&video, &video_id, &video_length,
                             &video_payload)) {
          break;
        }
        if (video_length <= 4 && video_id == 0xB0) {
          const uint64_t width = ReadUnsigned(
              video.data + video_payload, static_cast<size_t>(video_length));
          if (width > 0 && width <= 32768) state->width = static_cast<int>(width);
        } else if (video_length <= 4 && video_id == 0xBA) {
          const uint64_t height = ReadUnsigned(
              video.data + video_payload, static_cast<size_t>(video_length));
          if (height > 0 && height <= 32768) state->height = static_cast<int>(height);
        }
        video.pos = video_payload + static_cast<size_t>(video_length);
      }
    }
    cursor.pos = payload + static_cast<size_t>(element_length);
  }
}

inline void ParseWebm(const std::vector<uint8_t>& bytes,
                      MediaExtractorState* state) {
  if (state == nullptr || bytes.empty() || bytes.size() > kMediaExtractorMaxBytes)
    return;
  EbmlCursor root{bytes.data(), bytes.size(), 0};
  while (root.pos < root.size) {
    uint64_t id = 0;
    uint64_t element_length = 0;
    size_t payload = 0;
    if (!ReadEbmlElement(&root, &id, &element_length, &payload)) break;
    const uint8_t* data = root.data + payload;
    if (id == 0x18538067) {  // Segment
      // Keep parsing in-place so a malformed file cannot trigger unbounded
      // allocation or recursive copies.
      EbmlCursor segment{data, static_cast<size_t>(element_length), 0};
      while (segment.pos < segment.size) {
        uint64_t child_id = 0;
        uint64_t child_length = 0;
        size_t child_payload = 0;
        if (!ReadEbmlElement(&segment, &child_id, &child_length,
                             &child_payload)) {
          break;
        }
        // Reparse a bounded child through a tiny view.  The same parser is
        // deliberately iterative at the root; Segment is normally the only
        // nesting level used by WebM.
        const uint8_t* child = segment.data + child_payload;
        if (child_id == 0x1549A966) {  // Info
          EbmlCursor info{child, static_cast<size_t>(child_length), 0};
          while (info.pos < info.size) {
            uint64_t info_id = 0;
            uint64_t info_length = 0;
            size_t info_payload = 0;
            if (!ReadEbmlElement(&info, &info_id, &info_length,
                                 &info_payload)) {
              break;
            }
            if (info_id == 0x2AD7B1 && info_length > 0 && info_length <= 8) {
              const uint64_t scale = ReadUnsigned(
                  info.data + info_payload, static_cast<size_t>(info_length));
              if (scale > 0 && scale <= 1000000000ULL)
                state->timecode_scale = static_cast<int64_t>(scale);
            }
            info.pos = info_payload + static_cast<size_t>(info_length);
          }
        } else if (child_id == 0x1654AE6B) {  // Tracks
          EbmlCursor tracks{child, static_cast<size_t>(child_length), 0};
          while (tracks.pos < tracks.size) {
            uint64_t track_id = 0;
            uint64_t track_length = 0;
            size_t track_payload = 0;
            if (!ReadEbmlElement(&tracks, &track_id, &track_length,
                                 &track_payload)) {
              break;
            }
            if (track_id == 0xAE) {
              uint64_t number = 1;
              bool video = false;
              ParseWebmTrack(tracks.data + track_payload,
                             static_cast<size_t>(track_length), state, &number,
                             &video);
              if (video) {
                state->video_track_number = number;
                state->have_video_track = true;
              }
            }
            tracks.pos = track_payload + static_cast<size_t>(track_length);
          }
        } else if (child_id == 0x1F43B675) {  // Cluster
          EbmlCursor cluster{child, static_cast<size_t>(child_length), 0};
          int64_t cluster_time = 0;
          while (cluster.pos < cluster.size) {
            uint64_t cluster_id = 0;
            uint64_t cluster_length = 0;
            size_t cluster_payload = 0;
            if (!ReadEbmlElement(&cluster, &cluster_id, &cluster_length,
                                 &cluster_payload)) {
              break;
            }
            const uint8_t* block = cluster.data + cluster_payload;
            if (cluster_id == 0xE7 && cluster_length <= 8) {
              cluster_time = static_cast<int64_t>(ReadUnsigned(
                  block, static_cast<size_t>(cluster_length)));
            } else if (cluster_id == 0xA3 && cluster_length >= 4) {
              EbmlCursor block_cursor{block, static_cast<size_t>(cluster_length),
                                       0};
              uint64_t track = 0;
              size_t track_width = 0;
              if (ReadEbmlVint(&block_cursor, &track, &track_width, false) &&
                  track == state->video_track_number &&
                  block_cursor.pos <= block_cursor.size &&
                  block_cursor.size - block_cursor.pos >= 3) {
                int64_t relative = 0;
                ReadSigned16(block_cursor.data + block_cursor.pos, 2,
                             &relative);
                const uint8_t flags = block_cursor.data[block_cursor.pos + 2];
                block_cursor.pos += 3;
                // Lacing requires splitting the payload and is intentionally
                // rejected until a bounded implementation is needed.
                if ((flags & 0x06) == 0 && block_cursor.pos <= block_cursor.size &&
                    state->samples.size() < 1'000'000) {
                  MediaExtractorState::Sample sample;
                  sample.data.assign(block_cursor.data + block_cursor.pos,
                                     block_cursor.data + block_cursor.size);
                  sample.pts_us = ((cluster_time + relative) *
                                   state->timecode_scale) /
                                  1000;
                  sample.flags = (flags & 0x80) != 0 ? 1u : 0u;
                  state->samples.emplace_back(std::move(sample));
                }
              }
            }
            cluster.pos = cluster_payload + static_cast<size_t>(cluster_length);
          }
        }
        segment.pos = child_payload + static_cast<size_t>(child_length);
      }
      root.pos = payload + static_cast<size_t>(element_length);
      continue;
    }
    root.pos = payload + static_cast<size_t>(element_length);
  }
  state->duration_us = state->samples.empty() ? 0 : state->samples.back().pts_us;
  state->has_source = state->have_video_track && !state->samples.empty();
}

}  // namespace aim
