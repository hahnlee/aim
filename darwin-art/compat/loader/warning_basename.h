#pragma once
#include <cstring>
// GNU basename returns a suffix of the input, including an empty suffix after
// a trailing slash. Darwin's POSIX basename has a different ABI/semantics.
inline const char* darwin_art_linker_gnu_basename(const char* path) {
  const char* slash = std::strrchr(path, '/');
  return slash ? slash + 1 : path;
}
