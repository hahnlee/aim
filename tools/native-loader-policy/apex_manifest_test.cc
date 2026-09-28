#include "apexutil.h"
#include <cassert>
#include <iostream>

int main(int argc, char** argv) {
  assert(argc == 2);
  auto packages = android::apex::GetActivePackages(argv[1]);
  assert(packages.size() == 1);
  const auto& manifest = packages.begin()->second;
  assert(manifest.name() == "com.android.i18n");
  assert(manifest.version() > 0);
  std::cout << "AOSP active APEX reader PASS: " << manifest.name()
            << " version=" << manifest.version()
            << " provides=" << manifest.providenativelibs_size() << '\n';
}
