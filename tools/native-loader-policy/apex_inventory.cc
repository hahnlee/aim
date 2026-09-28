// Build-time inventory of an actually materialized root. Android protobuf and
// XML implementations own their formats; this adapter supplies placement only.
#include "apexutil.h"
#include "com_android_apex.h"
#include <filesystem>
#include <iostream>
#include <set>
#include <stdexcept>

namespace fs = std::filesystem;

int main(int argc, char** argv) {
  try {
    if (argc < 3) throw std::runtime_error("usage: apex-inventory ROOT MODULE=/system/apex/ARCHIVE ...");
    const fs::path root = argv[1];
    const auto packages = android::apex::GetActivePackages((root / "apex").string());
    if (packages.size() != static_cast<size_t>(argc - 2))
      throw std::runtime_error("selected package count differs from materialized manifests");
    std::set<std::string> selected;
    std::vector<com::android::apex::ApexInfo> records;
    for (int index = 2; index < argc; ++index) {
      const std::string entry = argv[index];
      const auto equal = entry.find('=');
      if (equal == std::string::npos) throw std::runtime_error("missing placement separator");
      const auto name = entry.substr(0, equal);
      const auto path = entry.substr(equal + 1);
      if (name.empty() || name == "." || name == ".." ||
          name.find_first_not_of("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-") != std::string::npos ||
          !selected.insert(name).second)
        throw std::runtime_error("invalid or repeated module name");
      const fs::path source = path;
      // This tool handles preinstalled SYSTEM archives only. Other partitions
      // require explicit installation ownership, not a default SYSTEM label.
      if (source.parent_path() != "/system/apex" || source.filename().empty() ||
          source.filename() == "." || source.filename() == ".." ||
          !fs::is_regular_file(fs::symlink_status(root / source.relative_path())))
        throw std::runtime_error("source must be an installed regular /system/apex archive");
      const auto found = packages.find((root / "apex" / name).string());
      if (found == packages.end() || found->second.name() != name || found->second.version() <= 0)
        throw std::runtime_error("module manifest identity does not match placement");
      const auto& manifest = found->second;
      records.emplace_back(name, path, path, manifest.version(), manifest.versionname(),
                           true, true, std::nullopt, manifest.providesharedapexlibs(), "SYSTEM");
    }
    com::android::apex::write(std::cout, com::android::apex::ApexInfoList(std::move(records)));
    if (!std::cout) throw std::runtime_error("inventory output failed");
    return 0;
  } catch (const std::exception& error) {
    std::cerr << "apex-inventory: " << error.what() << '\n';
    return 1;
  }
}
