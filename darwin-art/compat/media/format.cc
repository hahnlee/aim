#include "format.h"
#include <media/NdkMediaFormat.h>
#include <cstring>

#include <cstdint>
#include <string>
#include <unordered_map>
#include <utility>
#include <vector>

struct AMediaFormat {
  std::string description;
  std::string returned_string;
  std::unordered_map<std::string, int32_t> integers;
  std::unordered_map<std::string, int64_t> wide_integers;
  std::unordered_map<std::string, size_t> sizes;
  std::unordered_map<std::string, float> floats;
  std::unordered_map<std::string, std::string> strings;
  std::unordered_map<std::string, std::vector<uint8_t>> buffers;
};

#define MEDIA_KEY(name, value) extern "C" const char* name = value
MEDIA_KEY(AMEDIAFORMAT_KEY_BITRATE_MODE, "bitrate-mode");
MEDIA_KEY(AMEDIAFORMAT_KEY_BIT_RATE, "bitrate");
MEDIA_KEY(AMEDIAFORMAT_KEY_COLOR_FORMAT, "color-format");
MEDIA_KEY(AMEDIAFORMAT_KEY_COLOR_RANGE, "color-range");
MEDIA_KEY(AMEDIAFORMAT_KEY_COLOR_STANDARD, "color-standard");
MEDIA_KEY(AMEDIAFORMAT_KEY_COLOR_TRANSFER, "color-transfer");
MEDIA_KEY(AMEDIAFORMAT_KEY_FRAME_RATE, "frame-rate");
MEDIA_KEY(AMEDIAFORMAT_KEY_HEIGHT, "height");
MEDIA_KEY(AMEDIAFORMAT_KEY_I_FRAME_INTERVAL, "i-frame-interval");
MEDIA_KEY(AMEDIAFORMAT_KEY_LATENCY, "latency");
MEDIA_KEY(AMEDIAFORMAT_KEY_LEVEL, "level");
MEDIA_KEY(AMEDIAFORMAT_KEY_MIME, "mime");
MEDIA_KEY(AMEDIAFORMAT_KEY_PRIORITY, "priority");
MEDIA_KEY(AMEDIAFORMAT_KEY_PROFILE, "profile");
MEDIA_KEY(AMEDIAFORMAT_KEY_SLICE_HEIGHT, "slice-height");
MEDIA_KEY(AMEDIAFORMAT_KEY_STRIDE, "stride");
MEDIA_KEY(AMEDIAFORMAT_KEY_TEMPORAL_LAYERING, "ts-schema");
MEDIA_KEY(AMEDIAFORMAT_KEY_WIDTH, "width");
MEDIA_KEY(AMEDIAFORMAT_KEY_AAC_PROFILE, "aac-profile");
MEDIA_KEY(AMEDIAFORMAT_KEY_CHANNEL_COUNT, "channel-count");
MEDIA_KEY(AMEDIAFORMAT_KEY_CHANNEL_MASK, "channel-mask");
MEDIA_KEY(AMEDIAFORMAT_KEY_DURATION, "durationUs");
MEDIA_KEY(AMEDIAFORMAT_KEY_FLAC_COMPRESSION_LEVEL, "flac-compression-level");
MEDIA_KEY(AMEDIAFORMAT_KEY_IS_ADTS, "is-adts");
MEDIA_KEY(AMEDIAFORMAT_KEY_IS_AUTOSELECT, "is-autoselect");
MEDIA_KEY(AMEDIAFORMAT_KEY_IS_DEFAULT, "is-default");
MEDIA_KEY(AMEDIAFORMAT_KEY_IS_FORCED_SUBTITLE, "is-forced-subtitle");
MEDIA_KEY(AMEDIAFORMAT_KEY_LANGUAGE, "language");
MEDIA_KEY(AMEDIAFORMAT_KEY_MAX_HEIGHT, "max-height");
MEDIA_KEY(AMEDIAFORMAT_KEY_MAX_INPUT_SIZE, "max-input-size");
MEDIA_KEY(AMEDIAFORMAT_KEY_MAX_WIDTH, "max-width");
MEDIA_KEY(AMEDIAFORMAT_KEY_PUSH_BLANK_BUFFERS_ON_STOP, "push-blank-buffers-on-shutdown");
MEDIA_KEY(AMEDIAFORMAT_KEY_REPEAT_PREVIOUS_FRAME_AFTER, "repeat-previous-frame-after");
MEDIA_KEY(AMEDIAFORMAT_KEY_SAMPLE_RATE, "sample-rate");
MEDIA_KEY(AMEDIAFORMAT_KEY_ROTATION, "rotation-degrees");
#undef MEDIA_KEY

// An Android format entry has one type. Replacing it must invalidate the old
// typed value, while storage returned by getString remains owned by the format.
static void EraseEntry(AMediaFormat* format, const char* name) {
  format->integers.erase(name);
  format->wide_integers.erase(name);
  format->sizes.erase(name);
  format->floats.erase(name);
  format->strings.erase(name);
  format->buffers.erase(name);
}

extern "C" AMediaFormat* AMediaFormat_new() { return new AMediaFormat; }

extern "C" media_status_t AMediaFormat_delete(AMediaFormat* format) {
  delete format;
  return AMEDIA_OK;
}

extern "C" const char* AMediaFormat_toString(AMediaFormat* format) {
  if (format == nullptr) return nullptr;
  auto& description = format->description;
  description.clear();
  description = "AMediaFormat{";
  bool first = true;
  for (const auto& entry : format->strings) {
    if (!first) description += ", ";
    first = false;
    description += entry.first;
    description += "=";
    description += entry.second;
  }
  description += "}";
  return description.c_str();
}

extern "C" bool AMediaFormat_getInt32(AMediaFormat* format,
                                       const char* name,
                                       int32_t* output) {
  if (format == nullptr || name == nullptr || output == nullptr) return false;
  const auto found = format->integers.find(name);
  if (found == format->integers.end()) return false;
  *output = found->second;
  return true;
}

extern "C" bool AMediaFormat_getInt64(AMediaFormat* format,
                                       const char* name, int64_t* output) {
  if (format == nullptr || name == nullptr || output == nullptr) return false;
  const auto found = format->wide_integers.find(name);
  if (found == format->wide_integers.end()) return false;
  *output = found->second;
  return true;
}

extern "C" bool AMediaFormat_getFloat(AMediaFormat* format,
                                       const char* name, float* output) {
  if (format == nullptr || name == nullptr || output == nullptr) return false;
  const auto found = format->floats.find(name);
  if (found == format->floats.end()) return false;
  *output = found->second;
  return true;
}

extern "C" bool AMediaFormat_getSize(AMediaFormat* format,
                                      const char* name, size_t* output) {
  if (format == nullptr || name == nullptr || output == nullptr) return false;
  const auto found = format->sizes.find(name);
  if (found == format->sizes.end()) return false;
  *output = found->second;
  return true;
}

extern "C" void AMediaFormat_setSize(AMediaFormat* format,
                                     const char* name, size_t value) {
  if (format == nullptr || name == nullptr) return;
  EraseEntry(format, name);
  format->sizes[name] = value;
}

extern "C" bool AMediaFormat_getString(AMediaFormat* format,
                                        const char* name,
                                        const char** output) {
  if (format == nullptr || name == nullptr || output == nullptr) return false;
  const auto found = format->strings.find(name);
  if (found == format->strings.end()) return false;
  format->returned_string = found->second;
  *output = format->returned_string.c_str();
  return true;
}

extern "C" void AMediaFormat_setInt32(AMediaFormat* format,
                                       const char* name,
                                       int32_t value) {
  if (format != nullptr && name != nullptr) {
    EraseEntry(format, name);
    format->integers[name] = value;
  }
}

extern "C" void AMediaFormat_setInt64(AMediaFormat* format,
                                       const char* name, int64_t value) {
  if (format != nullptr && name != nullptr) {
    EraseEntry(format, name);
    format->wide_integers[name] = value;
  }
}

extern "C" void AMediaFormat_setFloat(AMediaFormat* format,
                                       const char* name,
                                       float value) {
  if (format != nullptr && name != nullptr) {
    EraseEntry(format, name);
    format->floats[name] = value;
  }
}

extern "C" void AMediaFormat_setString(AMediaFormat* format,
                                        const char* name,
                                        const char* value) {
  if (format != nullptr && name != nullptr && value != nullptr) {
    std::string owned(value);
    EraseEntry(format, name);
    format->strings[name] = std::move(owned);
  }
}

extern "C" bool AMediaFormat_getBuffer(AMediaFormat* format, const char* name,
                                         void** output, size_t* size) {
  if (format == nullptr || name == nullptr || output == nullptr || size == nullptr)
    return false;
  auto found = format->buffers.find(name);
  if (found == format->buffers.end()) return false;
  *output = found->second.data();
  *size = found->second.size();
  return true;
}

extern "C" void AMediaFormat_setBuffer(AMediaFormat* format, const char* name,
                                         const void* data, size_t size) {
  if (format == nullptr || name == nullptr || data == nullptr) return;
  const auto* bytes = static_cast<const uint8_t*>(data);
  std::vector<uint8_t> owned(bytes, bytes + size);
  EraseEntry(format, name);
  format->buffers[name] = std::move(owned);
}

namespace darwin_art::media {
void* FormatSymbol(const char* symbol) {
  if (symbol == nullptr) return nullptr;
#define MEDIA_FUNCTION(name) \
  if (std::strcmp(symbol, #name) == 0) return reinterpret_cast<void*>(&name);
  MEDIA_FUNCTION(AMediaFormat_delete)
  MEDIA_FUNCTION(AMediaFormat_getInt32)
  MEDIA_FUNCTION(AMediaFormat_getInt64)
  MEDIA_FUNCTION(AMediaFormat_getFloat)
  MEDIA_FUNCTION(AMediaFormat_getSize)
  MEDIA_FUNCTION(AMediaFormat_getString)
  MEDIA_FUNCTION(AMediaFormat_getBuffer)
  MEDIA_FUNCTION(AMediaFormat_new)
  MEDIA_FUNCTION(AMediaFormat_setFloat)
  MEDIA_FUNCTION(AMediaFormat_setInt32)
  MEDIA_FUNCTION(AMediaFormat_setInt64)
  MEDIA_FUNCTION(AMediaFormat_setBuffer)
  MEDIA_FUNCTION(AMediaFormat_setString)
  MEDIA_FUNCTION(AMediaFormat_toString)
  MEDIA_FUNCTION(AMediaFormat_setSize)
#undef MEDIA_FUNCTION
#define MEDIA_DATA(name)                             \
  if (std::strcmp(symbol, #name) == 0) {             \
    return reinterpret_cast<void*>(&name);           \
  }
  MEDIA_DATA(AMEDIAFORMAT_KEY_BITRATE_MODE)
  MEDIA_DATA(AMEDIAFORMAT_KEY_BIT_RATE)
  MEDIA_DATA(AMEDIAFORMAT_KEY_COLOR_FORMAT)
  MEDIA_DATA(AMEDIAFORMAT_KEY_COLOR_RANGE)
  MEDIA_DATA(AMEDIAFORMAT_KEY_COLOR_STANDARD)
  MEDIA_DATA(AMEDIAFORMAT_KEY_COLOR_TRANSFER)
  MEDIA_DATA(AMEDIAFORMAT_KEY_FRAME_RATE)
  MEDIA_DATA(AMEDIAFORMAT_KEY_HEIGHT)
  MEDIA_DATA(AMEDIAFORMAT_KEY_I_FRAME_INTERVAL)
  MEDIA_DATA(AMEDIAFORMAT_KEY_LATENCY)
  MEDIA_DATA(AMEDIAFORMAT_KEY_LEVEL)
  MEDIA_DATA(AMEDIAFORMAT_KEY_MIME)
  MEDIA_DATA(AMEDIAFORMAT_KEY_PRIORITY)
  MEDIA_DATA(AMEDIAFORMAT_KEY_PROFILE)
  MEDIA_DATA(AMEDIAFORMAT_KEY_SLICE_HEIGHT)
  MEDIA_DATA(AMEDIAFORMAT_KEY_STRIDE)
  MEDIA_DATA(AMEDIAFORMAT_KEY_TEMPORAL_LAYERING)
  MEDIA_DATA(AMEDIAFORMAT_KEY_WIDTH)
  MEDIA_DATA(AMEDIAFORMAT_KEY_AAC_PROFILE)
  MEDIA_DATA(AMEDIAFORMAT_KEY_CHANNEL_COUNT)
  MEDIA_DATA(AMEDIAFORMAT_KEY_CHANNEL_MASK)
  MEDIA_DATA(AMEDIAFORMAT_KEY_DURATION)
  MEDIA_DATA(AMEDIAFORMAT_KEY_FLAC_COMPRESSION_LEVEL)
  MEDIA_DATA(AMEDIAFORMAT_KEY_IS_ADTS)
  MEDIA_DATA(AMEDIAFORMAT_KEY_IS_AUTOSELECT)
  MEDIA_DATA(AMEDIAFORMAT_KEY_IS_DEFAULT)
  MEDIA_DATA(AMEDIAFORMAT_KEY_IS_FORCED_SUBTITLE)
  MEDIA_DATA(AMEDIAFORMAT_KEY_LANGUAGE)
  MEDIA_DATA(AMEDIAFORMAT_KEY_MAX_HEIGHT)
  MEDIA_DATA(AMEDIAFORMAT_KEY_MAX_INPUT_SIZE)
  MEDIA_DATA(AMEDIAFORMAT_KEY_MAX_WIDTH)
  MEDIA_DATA(AMEDIAFORMAT_KEY_PUSH_BLANK_BUFFERS_ON_STOP)
  MEDIA_DATA(AMEDIAFORMAT_KEY_REPEAT_PREVIOUS_FRAME_AFTER)
  MEDIA_DATA(AMEDIAFORMAT_KEY_SAMPLE_RATE)
  MEDIA_DATA(AMEDIAFORMAT_KEY_ROTATION)
#undef MEDIA_DATA
  return nullptr;
}
}  // namespace darwin_art::media
