#include "public_libraries.h"
#include <nativebridge/native_bridge.h>
#include "darwin_art_linker_namespace.h"
#include <iostream>
#include <sstream>
extern "C" int darwin_art_linker_dlclose(void*);
// Diagnostic only. Never replaces InitializeNativeLoader, never edits its list,
// and a missing library produces failure rather than an initialization PASS.
int AuditSystemPreloads() {
  std::istringstream names(android::nativeloader::preloadable_public_libraries());
  std::string name;
  size_t loaded = 0, failed = 0;
  while (std::getline(names, name, ':')) {
    void* handle = android::OpenSystemLibrary(name.c_str(), 0x1002);
    if (!handle) {
      const char* error = darwin_art_linker_dlerror();
      std::cerr << "PRELOAD FAIL " << name << ": " << (error ? error : "no linker error") << '\n';
      ++failed;
    } else {
      std::cout << "PRELOAD OPEN " << name << '\n';
      ++loaded;
      if (darwin_art_linker_dlclose(handle) != 0) {
        std::cerr << "PRELOAD CLOSE FAIL " << name << '\n';
        ++failed;
      }
    }
  }
  std::cout << "PRELOAD AUDIT loaded=" << loaded << " failed=" << failed << '\n';
  return failed ? 1 : 0;
}
