#include <cstddef>

#include "base/os.h"
#include "base/unix_file/fd_file.h"

extern "C" void* aim_upstream_open_file_for_reading(const char* path);
extern "C" bool aim_upstream_fd_file_read_fully(void* file,
                                                        void* buffer,
                                                        size_t size);

namespace art {

File* OS::OpenFileForReading(const char* path) {
  return static_cast<File*>(aim_upstream_open_file_for_reading(path));
}

}  // namespace art

namespace unix_file {

bool FdFile::ReadFully(void* buffer, size_t size) {
  return aim_upstream_fd_file_read_fully(this, buffer, size);
}

}  // namespace unix_file
