#include "provider_metadata.h"
#include <cstdint>

namespace darwin_art::framework::pm {
namespace {
int Nibble(char value) {
  if (value >= '0' && value <= '9') return value - '0';
  if (value >= 'a' && value <= 'f') return value - 'a' + 10;
  if (value >= 'A' && value <= 'F') return value - 'A' + 10;
  return -1;
}
bool Integer(const std::string& value) {
  if (value.empty() || value.size() > 8) return false;
  for (char digit : value) if (Nibble(digit) < 0) return false;
  return true;
}
}

bool DecodeHex(const std::string& encoded, std::string* decoded) {
  if (decoded == nullptr || encoded.size() % 2 != 0) return false;
  std::string result;
  result.reserve(encoded.size() / 2);
  for (size_t i = 0; i < encoded.size(); i += 2) {
    const int high = Nibble(encoded[i]), low = Nibble(encoded[i + 1]);
    if (high < 0 || low < 0) return false;
    result.push_back(static_cast<char>((high << 4) | low));
  }
  *decoded = std::move(result);
  return true;
}

std::vector<std::string> Split(const std::string& value, char delimiter) {
  std::vector<std::string> result;
  size_t begin = 0;
  while (begin <= value.size()) {
    const size_t end = value.find(delimiter, begin);
    result.push_back(value.substr(begin, end == std::string::npos ? end : end - begin));
    if (end == std::string::npos) break;
    begin = end + 1;
  }
  return result;
}

bool ValidateProviderMetadata(const char* encoded) {
  if (encoded == nullptr || *encoded == '\0' || std::string(encoded) == "none") return true;
  for (const auto& item : Split(encoded, ';')) {
    const auto fields = Split(item, '>');
    if (fields.size() != 5 || !Integer(fields[2]) ||
        (fields[3] != "0" && fields[3] != "1")) {
      return false;
    }
    std::string text;
    if (!DecodeHex(fields[0], &text) || text.empty() || text.find('\0') != std::string::npos) return false;
    if (!DecodeHex(fields[1], &text) || text.empty() || text.find('\0') != std::string::npos) return false;
    if (fields[4] == "none") continue;
    for (const auto& entry : Split(fields[4], ',')) {
      const auto parts = Split(entry, ':');
      if (parts.size() != 3 || !DecodeHex(parts[0], &text) || text.empty()
          || text.find('\0') != std::string::npos) return false;
      if (parts[1] == "s") {
        if (!DecodeHex(parts[2], &text) || text.find('\0') != std::string::npos) return false;
      } else if (parts[1] == "i" || parts[1] == "r") {
        if (!Integer(parts[2])) return false;
      } else if (parts[1] == "b") {
        if (parts[2] != "0" && parts[2] != "1") return false;
      } else return false;
    }
  }
  return true;
}
}
