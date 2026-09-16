#pragma once

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

enum { DARWIN_ART_LIBLOG_PROVIDER_COUNT = 19 };

size_t darwin_art_liblog_provider_count(void);
const char* darwin_art_liblog_provider_name(uint32_t ordinal);
/* Host-ABI backend address. Do not publish directly as an Android ELF symbol. */
uintptr_t darwin_art_liblog_provider_address(uint32_t ordinal);

/* Accept unversioned default imports or the symbol's pinned Android ELF version.
 * A version belonging to another symbol is rejected. */
uintptr_t darwin_art_liblog_provider_resolve(const char* symbol, const char* version);

#ifdef __cplusplus
}
#endif
