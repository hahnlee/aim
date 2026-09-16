#include "loader/boot_apex_jni_policy.h"

#include <cassert>
#include <filesystem>
#include <fstream>
#include <string>

int main(int argc, char** argv) {
  assert(argc == 2);
  const std::filesystem::path root(argv[1]);
  std::filesystem::create_directories(root / "linkerconfig");
  std::filesystem::create_directories(root / "apex/com.android.conscrypt/lib64");
  std::filesystem::create_directories(root / "apex/com.android.art/lib64");
  std::ofstream(root / "linkerconfig/apex.libraries.config.txt")
      << "jni com_android_conscrypt libjavacrypto.so\n"
      << "public com_android_art libnativehelper.so\n";
  std::ofstream(root / "apex/com.android.conscrypt/lib64/libjavacrypto.so")
      << "ELF fixture";
  std::ofstream(root / "apex/com.android.art/lib64/libnativehelper.so")
      << "ELF fixture";

  using darwin_art::loader::BootApexJniDecision;
  using darwin_art::loader::ResolveBootApexJniLibrary;
  using darwin_art::loader::ResolvePublicApexLibrary;
  const std::string root_string = root.string();
  auto allowed = ResolveBootApexJniLibrary(
      "libjavacrypto.so", "/apex/com.android.conscrypt/javalib/conscrypt.jar",
      root_string.c_str());
  assert(allowed.decision == BootApexJniDecision::kAllowed);
  assert(allowed.path == root_string +
                             "/apex/com.android.conscrypt/lib64/libjavacrypto.so");
  auto apk = ResolveBootApexJniLibrary(
      "libjavacrypto.so", "/data/app/example/base.apk", root_string.c_str());
  assert(apk.decision == BootApexJniDecision::kNotApplicable);
  auto wrong_module = ResolveBootApexJniLibrary(
      "libjavacrypto.so", "/apex/com.android.i18n/javalib/core-icu4j.jar",
      root_string.c_str());
  assert(wrong_module.decision == BootApexJniDecision::kDenied);
  auto traversal = ResolveBootApexJniLibrary(
      "../libjavacrypto.so", "/apex/com.android.conscrypt/javalib/conscrypt.jar",
      root_string.c_str());
  assert(traversal.decision == BootApexJniDecision::kDenied);
  assert(ResolvePublicApexLibrary("libnativehelper.so", root_string.c_str()) ==
         root_string + "/apex/com.android.art/lib64/libnativehelper.so");
  assert(ResolvePublicApexLibrary("libprivate.so", root_string.c_str()).empty());
  return 0;
}
