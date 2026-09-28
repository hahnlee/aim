#pragma once

#include "jit/jit.h"

// Same C++ namespace/linkage as compiler/export/jit_create.h. This declaration
// also permits the interpreter-only runtime objects to build without compiler
// headers; JIT-enabled linking must supply the real AOSP compiler definition.
namespace art::jit {
JitCompilerInterface* jit_create();
}
