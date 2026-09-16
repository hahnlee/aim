#pragma once
#include "darwin_art_linker_namespace.h"
#include <string>
namespace darwin_art::loader {
// Serializes the complete close operation. Guest handle is never dereferenced.
int CloseNamespaceLibrary(LinkerRegistry*, uintptr_t handle, std::string* error);
}
