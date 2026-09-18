#pragma once

#include <cstdlib>
#include <fstream>
#include <iterator>
#include <string>

namespace darwin_art_elf_probe {
inline std::string JournalPath() {
  const char* root = std::getenv("DARWIN_ART_ANDROID_PRIVATE_DATA_ROOT");
  return root == nullptr ? std::string{} : std::string(root) + "/jni-lifecycle.journal";
}
inline bool ResetJournal() {
  const std::string path = JournalPath();
  if (path.empty()) return false;
  std::ofstream stream(path, std::ios::trunc);
  return stream.good();
}
inline std::string ReadJournal() {
  std::ifstream stream(JournalPath());
  return std::string(std::istreambuf_iterator<char>(stream),
                     std::istreambuf_iterator<char>());
}
}  // namespace darwin_art_elf_probe
