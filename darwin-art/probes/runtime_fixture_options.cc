#include "runtime_fixture_options.h"

#include <sys/stat.h>

#include <cstdlib>

namespace darwin_art_process {
namespace {

std::string Env(const char* name) {
  const char* value = std::getenv(name);
  return value == nullptr ? std::string() : std::string(value);
}

bool Present(const std::string& value) { return !value.empty(); }

bool IsSha256(const std::string& value) {
  if (value.size() != 64) return false;
  for (unsigned char byte : value) {
    if (!((byte >= '0' && byte <= '9') || (byte >= 'a' && byte <= 'f'))) {
      return false;
    }
  }
  return true;
}

bool IsPrivateExtractedRoot(const std::string& path) {
  if (path.empty()) return false;
  struct stat path_stat {};
  struct stat followed_stat {};
  if (lstat(path.c_str(), &path_stat) != 0 ||
      stat(path.c_str(), &followed_stat) != 0 ||
      !S_ISREG(path_stat.st_mode) || path_stat.st_dev != followed_stat.st_dev ||
      path_stat.st_ino != followed_stat.st_ino ||
      (path_stat.st_mode & 0777) != 0400) {
    return false;
  }
  const std::size_t separator = path.rfind('/');
  if (separator == std::string::npos || separator == 0) return false;
  struct stat directory_stat {};
  const std::string directory = path.substr(0, separator);
  return lstat(directory.c_str(), &directory_stat) == 0 &&
         S_ISDIR(directory_stat.st_mode) &&
         (directory_stat.st_mode & 0777) == 0500;
}

}  // namespace

int LoadRuntimeFixtureOptions(FixtureOptions* options, std::string* error) {
  if (options == nullptr || error == nullptr) return 48;
  *options = FixtureOptions{};
  options->run_framework_button =
      std::getenv("DARWIN_ART_APK_APP_PACKAGE") == nullptr &&
      std::getenv("DARWIN_ART_APK_APP_ACTIVITY") == nullptr &&
      std::getenv("DARWIN_ART_APK_APP_DESCRIPTOR") == nullptr &&
      std::getenv("DARWIN_ART_TEST_FONTS_XML") != nullptr &&
      Env("DARWIN_ART_FRAMEWORK_RES_APK").starts_with('/');
  options->expect_apk_widgets = Env("DARWIN_ART_APK_APP_EXPECT_WIDGETS") == "1";
  options->elf_fixture_path = Env("DARWIN_ART_ANDROID_ELF_JNI_FIXTURE");
  options->generic_elf_path = Env("DARWIN_ART_ANDROID_ELF_GENERIC_FIXTURE");
  options->apk_elf_path = Env("DARWIN_ART_ANDROID_APK_ELF_FIXTURE");
  options->apk_sha256 = Env("DARWIN_ART_ANDROID_APK_SHA256");
  options->apk_root_sha256 = Env("DARWIN_ART_ANDROID_APK_ROOT_SHA256");
  options->direct_apk_path = Env("DARWIN_ART_DIRECT_APK_FIXTURE");
  options->direct_apk_root = Env("DARWIN_ART_DIRECT_APK_ROOT");
  options->libcxx_collections_path =
      Env("DARWIN_ART_ANDROID_LIBCXX_COLLECTIONS_FIXTURE");
  options->libcxx_exception_path =
      Env("DARWIN_ART_ANDROID_LIBCXX_EXCEPTION_FIXTURE");
  options->tls_fixture_path = Env("DARWIN_ART_ANDROID_TLS_FIXTURE");
  options->network_fixture_path = Env("DARWIN_ART_ANDROID_NETWORK_FIXTURE");

  options->run_elf_jni_fixture = Present(options->elf_fixture_path);
  options->run_generic_elf = Present(options->generic_elf_path);
  options->run_apk_elf =
      Present(options->apk_elf_path) && IsSha256(options->apk_sha256) &&
      IsSha256(options->apk_root_sha256) &&
      IsPrivateExtractedRoot(options->apk_elf_path) &&
      options->run_generic_elf &&
      options->apk_elf_path == options->generic_elf_path;
  options->run_direct_apk = Present(options->direct_apk_path) &&
                            Present(options->direct_apk_root);
  options->run_libcxx_acceptance =
      Present(options->libcxx_collections_path) &&
      Present(options->libcxx_exception_path);
  options->run_tls_acceptance = Present(options->tls_fixture_path);
  options->run_network_acceptance = Present(options->network_fixture_path);

  if (options->run_network_acceptance &&
      (options->run_elf_jni_fixture || options->run_generic_elf ||
       options->run_apk_elf || options->run_libcxx_acceptance ||
       options->run_tls_acceptance || options->run_direct_apk)) {
    *error = "ART Android network fixture requires an isolated process";
    return 47;
  }
  return 0;
}

}  // namespace darwin_art_process
