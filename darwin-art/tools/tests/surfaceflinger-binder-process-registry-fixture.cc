#include <cstdint>
#include <unistd.h>

extern "C" int32_t darwin_art_runtime_registered_process_uid(uint32_t pid) {
  return pid == static_cast<uint32_t>(getpid()) ? 1000 : -1;
}
