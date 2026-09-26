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

// The host's effective proxy configuration (SCDynamicStoreCopyProxies), as
// Android's ProxyInfo sees it: none, one HTTP(S) proxy or a PAC URL.
constexpr uint32_t kDarwinArtNetworkProxyNone = 0;
constexpr uint32_t kDarwinArtNetworkProxyDirect = 1;
constexpr uint32_t kDarwinArtNetworkProxyPac = 2;

struct DarwinArtNetworkProxyFacts {
  uint32_t abi_version;
  uint32_t struct_size;
  uint32_t kind;
  uint32_t port;
  char host[256];
  char pac_url[1024];
  // Comma-separated host patterns that bypass the proxy.
  char exclusions[2048];
};

int darwin_art_network_proxy_snapshot(DarwinArtNetworkProxyFacts* output);

void* darwin_art_network_path_create();
int darwin_art_network_path_snapshot(
    const void* handle, DarwinArtNetworkPathSnapshot* output);
void darwin_art_network_path_destroy(void* handle);

}  // extern "C"
