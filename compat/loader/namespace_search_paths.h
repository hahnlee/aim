#pragma once
#include <string>
namespace aim::loader {
// Original Bionic split/resolve with guest filesystem authority.
std::string ResolveLibraryPaths(const char* paths);
}
