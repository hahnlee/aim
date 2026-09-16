#pragma once
#include <string>
namespace darwin_art::loader {
// Original Bionic split/resolve with guest filesystem authority.
std::string ResolveLibraryPaths(const char* paths);
}
