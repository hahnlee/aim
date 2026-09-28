#include "runtime_filesystem_probe.h"

#include <fcntl.h>
#include <unistd.h>

#include <cstdint>
#include <cstdlib>
#include <iostream>

#include "aim_bionic_fs.h"

bool InstallProbeAndroidSystemRoot() {
  const char* filesystem_root =
      std::getenv("AIM_ANDROID_FILESYSTEM_ROOT");
  const char* root = filesystem_root != nullptr
                         ? filesystem_root
                         : std::getenv("AIM_ANDROID_SYSTEM_ROOT");
  if (root == nullptr || root[0] == '\0') return true;
  const int fd = open(root, O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW);
  if (fd < 0) {
    std::cerr << "ART Android filesystem: cannot open test root " << root << "\n";
    return false;
  }
  constexpr uint8_t kSystemMount[] = {'/', 's', 'y', 's', 't', 'e', 'm'};
  constexpr uint8_t kRootMount[] = {'/'};
  const uint8_t* mount = filesystem_root != nullptr ? kRootMount : kSystemMount;
  const size_t mount_size =
      filesystem_root != nullptr ? sizeof(kRootMount) : sizeof(kSystemMount);
  const auto status = aim_bionic_fs_process_install(
      fd, mount, mount_size, mount, mount_size);
  close(fd);
  std::cerr << "ART Android filesystem: test root status="
            << static_cast<int>(status) << " root=" << root << "\n";
  return status == AIM_BIONIC_FS_PROCESS_OWNER_OK ||
         status == AIM_BIONIC_FS_PROCESS_OWNER_ALREADY_INSTALLED;
}
