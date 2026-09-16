#include "public_libraries.h"
#include "filesystem/guest_config.h"
#include "filesystem/guest_directory.h"
#include "filesystem/guest_file.h"
#include "filesystem/linker_config_fs.h"
#include <cerrno>
#include <cstring>
#include <sys/stat.h>
#include "darwin_art_bionic_process_state.h"
#include <android/api-level.h>
#include <sys/system_properties.h>
#include <android-base/properties.h>
#include <android/sysprop/VndkProperties.sysprop.h>
#include <cassert>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <fcntl.h>
#include <unistd.h>
int main() {
  char root[] = "/tmp/darwin-policy-config.XXXXXX";
  assert(mkdtemp(root));
  namespace fs = std::filesystem;
  const auto private_root = fs::path(root) / "private-data";
  fs::create_directories(private_root);
  setenv("DARWIN_ART_ANDROID_PRIVATE_DATA_ROOT", private_root.c_str(), 1);
  std::ofstream(private_root / "comm") << "abcdefgh";
  fs::create_directories(fs::path(root) / "system/etc");
  fs::create_directories(fs::path(root) / "linkerconfig");
  fs::create_directories(fs::path(root) / "system/lib64");
  std::ofstream(fs::path(root) / "system/lib64/libsobridge.so") << "guest-only";
  fs::create_symlink("/system/etc", fs::path(root) / "guest-link");
  std::ofstream(fs::path(root) / "system/etc/public.libraries.txt") << "libtest.so\n";
  std::ofstream(fs::path(root) / "linkerconfig/apex.libraries.config.txt") << "# empty\n";
  std::ofstream(fs::path(root) / "system/etc/public.libraries-acme.txt") << "libdemo.acme.so\n";
  int fd = open(root, O_RDONLY | O_DIRECTORY | O_CLOEXEC);
  assert(fd >= 0);
  assert(darwin_art_bionic_fs_process_install(fd,
      reinterpret_cast<const uint8_t*>("/"), 1,
      reinterpret_cast<const uint8_t*>("/"), 1) == 0);
  close(fd);
  assert(darwin_art_bionic_process_state_process_install() == 0);
  char sdk[92] = {};
  assert(__system_property_get("ro.build.version.sdk", sdk) > 0);
  assert(android_get_device_api_level() == std::stoi(sdk));
  assert(android::base::GetProperty("ro.build.version.sdk", "missing") == sdk);
  assert(android::base::GetIntProperty("ro.build.version.sdk", -1) == std::stoi(sdk));
  assert(android::base::GetProperty("no.such.property", "fallback") == "fallback");
  // Generated getters must use the same device property owner as SDK queries.
  // The installed snapshot has no VNDK override; host-map writes are not a
  // substitute for an authorized property-service update.
  assert(!android::sysprop::VndkProperties::vendor_vndk_version().has_value());
  assert(!android::sysprop::VndkProperties::product_vndk_version().has_value());
  android::base::CachedProperty cached_sdk("ro.build.version.sdk");
  assert(std::string(cached_sdk.Get()) == sdk);
  assert(std::string(cached_sdk.Get()) == sdk);
  assert(cached_sdk.WaitForChange(std::chrono::milliseconds(0)) == nullptr);
  assert(android::base::WaitForProperty("ro.build.version.sdk", sdk, std::chrono::milliseconds(0)));
  assert(!android::base::WaitForProperty("ro.build.version.sdk", "not-sdk", std::chrono::milliseconds(0)));
  setenv("ANDROID_ROOT", "/system", 1);
  // Executes original policy parser + file reader against the real guest VFS.
  assert(android::nativeloader::default_public_libraries() == "libtest.so");
  assert(android::nativeloader::extended_public_libraries() == "libdemo.acme.so");
  {
    using GuestDirectory = darwin_art::filesystem::GuestDirectory;
    auto guest_directory = GuestDirectory::Open("/guest-link");
    assert(guest_directory);
    DarwinArtAndroidDirent entry{};
    bool guest_found = false;
    for (;;) {
      const auto result = guest_directory->Next(&entry);
      assert(result != GuestDirectory::Result::Error);
      if (result == GuestDirectory::Result::End) break;
      if (std::string(entry.d_name) == "public.libraries-acme.txt") {
        guest_found = true;
        assert(entry.d_type == 8); // Android DT_REG.
      }
    }
    assert(guest_found);
    assert(guest_directory->Next(&entry) == GuestDirectory::Result::End);
    assert(guest_directory->Next(nullptr) == GuestDirectory::Result::Error && errno == EINVAL);
    assert(!GuestDirectory::Open("/missing") && errno == ENOENT);
    assert(!GuestDirectory::Open("/system/etc/public.libraries.txt") && errno == ENOTDIR);
    assert(!GuestDirectory::Open("/etc") && errno == ENOENT); // No host fallback.
  }
  std::string bytes;
  {
    auto file = darwin_art::filesystem::GuestFile::Open("/data/comm", true);
    assert(file && file->ReadAll(&bytes) && bytes == "abcdefgh");
    fs::rename(private_root / "comm", private_root / "comm-old");
    std::ofstream(private_root / "comm") << "replacement";
    assert(file->WriteAtStart("XY"));
    assert(darwin_art::filesystem::ReadGuestConfig("/data/comm-old", &bytes));
    assert(bytes == "XYcdefgh"); // Same admitted file, prefix write, no truncation.
    assert(darwin_art::filesystem::ReadGuestConfig("/data/comm", &bytes));
    assert(bytes == "replacement");
  }
  {
    // Renaming/replacing the backing pathname cannot retarget an admitted FD.
    const auto path = fs::path(root) / "system/etc/retained.txt";
    const std::string content(10000, 'R');
    std::ofstream(path) << content;
    auto file = darwin_art::filesystem::GuestFile::Open("/system/etc/retained.txt");
    assert(file);
    fs::rename(path, fs::path(root) / "system/etc/retained-old.txt");
    std::ofstream(path) << "replacement";
    assert(file->ReadAll(&bytes) && bytes == content);
    assert(!file->WriteAtStart("forbidden") && errno == EBADF);
    assert(darwin_art::filesystem::ReadGuestConfig("/system/etc/retained.txt", &bytes));
    assert(bytes == "replacement");
  }
  assert(darwin_art::filesystem::ReadGuestConfig("/guest-link/public.libraries.txt", &bytes));
  assert(bytes == "libtest.so\n");
  // This regular virtual file is supplied by the Rust VFS, not the image tree.
  assert(!fs::exists(fs::path(root) / "sys/devices/system/cpu/online"));
  assert(darwin_art::filesystem::ReadGuestConfig("/sys/devices/system/cpu/online", &bytes));
  assert(!bytes.empty());
  assert(!darwin_art::filesystem::ReadGuestConfig("relative", &bytes) && errno == EINVAL);
  assert(!darwin_art::filesystem::ReadGuestConfig("/system/etc", &bytes) && errno == EISDIR);
  assert(!darwin_art::filesystem::ReadGuestConfig("/etc/passwd", &bytes) && errno == ENOENT);
  assert(!darwin_art::filesystem::ReadGuestConfig("/missing", &bytes));
  using namespace darwin_art::filesystem;
  char resolved[1024];
  assert(GuestLinkerRealpath("/guest-link", resolved) == resolved);
  assert(std::strcmp(resolved, "/system/etc") == 0);
  char small[2] = {'x', 'y'};
  assert(!GuestLinkerRealpath("/system/etc", small) && errno == ENAMETOOLONG);
  assert(small[0] == 'x' && small[1] == 'y');
  assert(GuestLinkerAccess("/system/etc", R_OK) == 0);
  assert(GuestLinkerAccess("/missing", F_OK) == -1 && errno == ENOENT);
  DarwinArtAndroidStat status{};
  assert(GuestLinkerStat("/guest-link", &status) == 0 && S_ISDIR(status.st_mode));
  assert(GuestLinkerStat("/missing", &status) == -1 && errno == ENOENT);
  // NativeLoader's original device workaround must inspect the guest image,
  // with Android's stat layout, not accidentally read macOS /system/lib64.
  assert(GuestLinkerStat("/system/lib64/libsobridge.so", &status) == 0);
  assert(S_ISREG(status.st_mode) && status.st_size == 10);
  assert(GuestLinkerStat("/system/lib64/libwalkstack.so", &status) == -1);
  assert(errno == ENOENT);
  assert(GuestLinkerStat("/etc/passwd", &status) == -1 && errno == ENOENT);
  assert(GuestLinkerAccess("/etc/passwd", R_OK) == -1); // no host fallback
  assert(darwin_art_bionic_fs_process_uninstall() == 0);
  assert(darwin_art_bionic_process_state_process_uninstall() == 0);
  unsetenv("DARWIN_ART_ANDROID_PRIVATE_DATA_ROOT");
  fs::remove_all(root);
}
