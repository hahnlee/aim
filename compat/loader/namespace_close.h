#pragma once
#include "aim_linker_namespace.h"
#include <string>
namespace aim::loader {
// Serializes the complete close operation. Guest handle is never dereferenced.
int CloseNamespaceLibrary(LinkerRegistry*, uintptr_t handle, std::string* error);
}
