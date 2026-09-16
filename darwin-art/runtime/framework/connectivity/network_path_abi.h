#pragma once

#include <cstdint>

extern "C" {

struct DarwinArtNetworkPathSnapshot {
  uint32_t abi_version;
  uint32_t struct_size;
  uint64_t generation;
  uint32_t status;
  uint32_t flags;
  uint32_t interface_mask;
  uint32_t reserved;
};

// This is a copied view of macOS's global default resolver and primary
// interface. It deliberately excludes per-service/scoped DNS dictionaries:
// those are a separate resolver policy and must not be flattened into Android.
constexpr uint32_t kDarwinArtNetworkMaxDnsServers = 8;
constexpr uint32_t kDarwinArtNetworkInterfaceNameCapacity = 64;
constexpr uint32_t kDarwinArtNetworkDnsAddressCapacity = 64;

struct DarwinArtNetworkLinkFacts {
  uint32_t abi_version;
  uint32_t struct_size;
  uint32_t dns_count;
  uint32_t reserved;
  char interface_name[kDarwinArtNetworkInterfaceNameCapacity];
  char dns_servers[kDarwinArtNetworkMaxDnsServers]
                  [kDarwinArtNetworkDnsAddressCapacity];
};

int darwin_art_network_link_facts_snapshot(DarwinArtNetworkLinkFacts* output);

void* darwin_art_network_path_create();
int darwin_art_network_path_snapshot(
    const void* handle, DarwinArtNetworkPathSnapshot* output);
void darwin_art_network_path_destroy(void* handle);

}  // extern "C"
