#include "public_libraries.h"
#include <cassert>
#include <iostream>
#include <fstream>
#include <sstream>

// Tests original parsers only. No production namespace
// backend or guest filesystem is substituted by this host-only executable.
int main(int argc, char** argv) {
  using namespace android::nativeloader::internal;
  const auto include_all = [](const ConfigEntry&) -> android::base::Result<bool> {
    return true;
  };
  auto parsed = ParseConfig("# comment\nliba.so\nlib32.so 32\nlib64.so 64\n"
                            "liblazy.so nopreload 64\n", include_all);
  assert(parsed.ok());
  assert((*parsed == std::vector<std::string>{"liba.so", "lib64.so", "liblazy.so"}));
  auto preload = ParseConfig("liba.so\nliblazy.so nopreload\n",
      [](const ConfigEntry& entry) -> android::base::Result<bool> {
        return !entry.nopreload;
      });
  assert(preload.ok() && *preload == std::vector<std::string>{"liba.so"});
  assert(!ParseConfig("libbad.so 32 64", include_all).ok());
  assert(!ParseConfig("libbad.so unknown", include_all).ok());
  auto denied = ParseConfig("liba.so", [](const ConfigEntry&) -> android::base::Result<bool> {
    return android::base::Error() << "filter denied";
  });
  assert(!denied.ok());
  auto apex = ParseApexLibrariesConfig(
      "public com_android_art liba.so:libb.so\njni com_android_art libjni.so\n", "public");
  assert(apex.ok() && apex->size() == 1 && apex->at("com_android_art") == "liba.so:libb.so");
  assert(!ParseApexLibrariesConfig("public missing_list", "public").ok());
  if (argc == 2) {
    std::ifstream input(argv[1]);
    assert(input.good());
    std::ostringstream content;
    content << input.rdbuf();
    auto public_map = ParseApexLibrariesConfig(content.str(), "public");
    auto jni_map = ParseApexLibrariesConfig(content.str(), "jni");
    assert(public_map.ok() && jni_map.ok());
    assert(public_map->at("com_android_art") == "libnativehelper.so");
    assert(public_map->at("com_android_i18n") == "libicui18n.so:libicuuc.so:libicu.so");
    assert(jni_map->at("com_android_conscrypt") == "libjavacrypto.so");
    assert(jni_map->at("com_android_art") == "libartservice.so");
    std::cout << "actual generated APEX policy parsed by original NativeLoader PASS\n";
  } else {
    assert(argc == 1);
  }
  std::cout << "original public-library and APEX parsers PASS\n";
}
