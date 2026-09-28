#include "namespace_loader_abi.h"
#include "aim_linker_namespace.h"

extern "C" char* __loader_dlerror() {
  // Consume only this thread's pending Android error. Do not consult dyld or
  // create a second diagnostic store for original libdl's forwarding ABI.
  return aim_linker_dlerror();
}
