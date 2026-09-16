#include "../compat/loader/library_search.h"

#include <cassert>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <unistd.h>

int main() {
  char scratch[] = "/tmp/darwin-library-search.XXXXXX";
  const char* created = mkdtemp(scratch);
  assert(created != nullptr);
  const std::filesystem::path root(created);
  std::filesystem::create_directories(root / "first");
  std::filesystem::create_directories(root / "second");
  const std::string first = (root / "first").string();
  const std::string second = (root / "second").string();
  std::ofstream(first + "/libsample.so").put('1');
  std::ofstream(second + "/libsample.so").put('2');
  std::filesystem::create_directory(first + "/libdirectory.so");
  using darwin_art::loader::FindLibraryInSearchPath;
  assert(FindLibraryInSearchPath("libsample.so", first + ":" + second) ==
         first + "/libsample.so");
  assert(FindLibraryInSearchPath("libsample.so", second + ":" + first) ==
         second + "/libsample.so");
  assert(FindLibraryInSearchPath("libsample.so", ":relative::" + second + ":") ==
         second + "/libsample.so");
  assert(FindLibraryInSearchPath("libdirectory.so", first).empty());
  assert(FindLibraryInSearchPath("../libsample.so", first).empty());
  assert(FindLibraryInSearchPath(first + "/libsample.so", first).empty());
  assert(FindLibraryInSearchPath(std::string("libsample.so\0junk", 17), first).empty());
  assert(FindLibraryInSearchPath("libsample.so", first + std::string(1, '\0')).empty());
  // A process-global app directory is not ClassLoader authority.
  const char* previous = std::getenv("DARWIN_ART_APK_APP_NATIVE_DIR");
  const bool had_previous = previous != nullptr;
  const std::string saved = previous == nullptr ? "" : previous;
  setenv("DARWIN_ART_APK_APP_NATIVE_DIR", first.c_str(), 1);
  assert(FindLibraryInSearchPath("libsample.so", "").empty());
  assert(FindLibraryInSearchPath("libsample.so", ":.").empty());
  if (had_previous) setenv("DARWIN_ART_APK_APP_NATIVE_DIR", saved.c_str(), 1);
  else unsetenv("DARWIN_ART_APK_APP_NATIVE_DIR");
  std::filesystem::remove_all(root);
  std::cout << "library search contract PASS\n";
}
