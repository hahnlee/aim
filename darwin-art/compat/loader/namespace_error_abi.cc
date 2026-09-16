#include "namespace_loader_abi.h"
#include "darwin_art_linker_namespace.h"

extern "C" char* __loader_dlerror() {
  // Consume only this thread's pending Android error. Do not consult dyld or
  // create a second diagnostic store for original libdl's forwarding ABI.
  return darwin_art_linker_dlerror();
}
