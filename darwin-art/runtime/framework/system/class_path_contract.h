#pragma once

#include <string>

namespace darwin_art::framework::system {

// Classpath and immutable image inputs are single path components.  Keep the
// grammar shared by the system-process owner and the embedding boundary so a
// checked value cannot become weaker when it crosses the process entry ABI.
inline bool IsValidSystemPath(const char* raw) {
  if (raw == nullptr) return false;
  const std::string path(raw);
  return path.size() > 1 && path.front() == '/' && path.back() != '/' &&
         path.find(':') == std::string::npos &&
         path.find("/../") == std::string::npos && !path.ends_with("/..") &&
         path.find("/./") == std::string::npos && !path.ends_with("/.");
}

}  // namespace darwin_art::framework::system
