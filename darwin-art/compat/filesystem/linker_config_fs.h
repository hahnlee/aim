#pragma once
#include "guest_config.h"
#include "darwin_art_bionic_stat.h"
#include <cstddef>
namespace darwin_art::filesystem {
int GuestLinkerAccess(const char*, int);
int GuestLinkerStat(const char*, DarwinArtAndroidStat*);
char* GuestLinkerRealpathBuffer(const char*, char*, size_t);
template<size_t N> char* GuestLinkerRealpath(const char* path, char (&output)[N]) {
  return GuestLinkerRealpathBuffer(path, output, N);
}
}
