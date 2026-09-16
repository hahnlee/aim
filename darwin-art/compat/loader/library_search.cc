#include "library_search.h"

#include <sys/stat.h>

namespace darwin_art::loader {

std::string FindLibraryInSearchPath(const std::string& soname,
                                    const std::string& search_path) {
  if (soname.empty() || soname == "." || soname == ".." ||
      soname.find('/') != std::string::npos ||
      soname.find('\\') != std::string::npos ||
      soname.find('\0') != std::string::npos ||
      search_path.find('\0') != std::string::npos) {
    return {};
  }
  size_t begin = 0;
  while (begin < search_path.size()) {
    const size_t end = search_path.find(':', begin);
    const std::string directory = search_path.substr(
        begin, end == std::string::npos ? end : end - begin);
    if (!directory.empty() && directory.front() == '/') {
      const std::string candidate = directory + "/" + soname;
      struct stat status {};
      if (stat(candidate.c_str(), &status) == 0 && S_ISREG(status.st_mode)) {
        return candidate;
      }
    }
    if (end == std::string::npos) break;
    begin = end + 1;
  }
  return {};
}

}  // namespace darwin_art::loader
