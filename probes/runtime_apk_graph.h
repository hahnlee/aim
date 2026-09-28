#ifndef AIM_RUNTIME_APK_GRAPH_H_
#define AIM_RUNTIME_APK_GRAPH_H_

#if defined(AIM_DIRECT_APK_RUNTIME)

#include "aim_elf_loader.h"

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

AimElfStatus aim_direct_discover_sibling_graph(
    int directory_fd, const uint8_t* root_component,
    size_t root_component_length, const char* const* provider_sonames,
    size_t provider_count, int* out_root_is_elf,
    AimElfDiscoveredGraph** out_graph, AimElfErrorBuffer* error);

AimElfStatus aim_direct_discovered_graph_root_soname(
    const AimElfDiscoveredGraph* graph, const char** out_soname,
    AimElfErrorBuffer* error);

AimElfStatus aim_direct_discovered_graph_sources(
    const AimElfDiscoveredGraph* graph,
    const AimElfGraphSource** out_sources, size_t* out_count,
    AimElfErrorBuffer* error);

void aim_direct_discovered_graph_destroy(
    AimElfDiscoveredGraph** graph);

#ifdef __cplusplus
}  // extern "C"
#endif

#endif  // defined(AIM_DIRECT_APK_RUNTIME)

#endif  // AIM_RUNTIME_APK_GRAPH_H_
