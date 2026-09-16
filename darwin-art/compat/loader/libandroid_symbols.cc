#include "libandroid_symbols.h"
#include "darwin_android_asset_manager.h"
#include "darwin_android_platform.h"
#include "darwin_android_system_fonts.h"
#include "network/multinetwork.h"
#include <cstring>
namespace darwin_art::loader {
int ResolveLibandroidPlatformSymbol(const char* symbol, const char* version, uintptr_t* output) {
  if (!output) return -1;
  *output = 0;
  if (!symbol || !*symbol || std::strncmp(symbol, "darwin_art_", 11) == 0)
    return 1;
  if (void* network = darwin_art_android_multinetwork_symbol(symbol, version)) {
    *output = reinterpret_cast<uintptr_t>(network);
    return 0;
  }
  if (version && *version) return 1;
  void* matches[] = {darwin_art_android_asset_manager_symbol(symbol),
      darwin_art_android_platform_symbol(symbol), darwin_art_android_system_font_symbol(symbol)};
  void* result = nullptr;
  for (void* match : matches) {
    if (!match) continue;
    if (result) return -1;
    result = match;
  }
  *output = reinterpret_cast<uintptr_t>(result);
  return result ? 0 : 1;
}
}
