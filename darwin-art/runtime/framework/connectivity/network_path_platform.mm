#import <Foundation/Foundation.h>
#import <Network/Network.h>
#import <SystemConfiguration/SystemConfiguration.h>

#include "network_path_abi.h"

#include <arpa/inet.h>
#include <cerrno>
#include <cstdint>
#include <cstring>

using Update = void (*)(void*, uint32_t, uint32_t, uint32_t);

@interface DarwinArtNetworkPathMonitor : NSObject
@property(nonatomic, strong) nw_path_monitor_t monitor;
@property(nonatomic, strong) dispatch_queue_t queue;
@property(nonatomic, assign) void* context;
@property(nonatomic, assign) Update update;
@end

@implementation DarwinArtNetworkPathMonitor
@end

namespace {

constexpr uint32_t kStatusUnknown = 0;
constexpr uint32_t kStatusSatisfied = 1;
constexpr uint32_t kStatusUnsatisfied = 2;
constexpr uint32_t kStatusRequiresConnection = 3;
constexpr uint32_t kFlagExpensive = 1u << 0;
constexpr uint32_t kFlagConstrained = 1u << 1;

uint32_t PathStatus(nw_path_t path) {
  switch (nw_path_get_status(path)) {
    case nw_path_status_satisfied:
      return kStatusSatisfied;
    case nw_path_status_unsatisfied:
      return kStatusUnsatisfied;
    case nw_path_status_satisfiable:
      return kStatusRequiresConnection;
    case nw_path_status_invalid:
      return kStatusUnknown;
  }
}

uint32_t InterfaceMask(nw_path_t path) {
  uint32_t result = 0;
  if (nw_path_uses_interface_type(path, nw_interface_type_wifi)) result |= 1u << 0;
  if (nw_path_uses_interface_type(path, nw_interface_type_wired)) result |= 1u << 1;
  if (nw_path_uses_interface_type(path, nw_interface_type_cellular)) result |= 1u << 2;
  if (nw_path_uses_interface_type(path, nw_interface_type_loopback)) result |= 1u << 3;
  if (nw_path_uses_interface_type(path, nw_interface_type_other)) result |= 1u << 4;
  return result;
}

}  // namespace

extern "C" void* darwin_art_network_path_platform_start(void* context,
                                                          Update update) {
  if (context == nullptr || update == nullptr) return nullptr;
  DarwinArtNetworkPathMonitor* owner = [[DarwinArtNetworkPathMonitor alloc] init];
  owner.context = context;
  owner.update = update;
  owner.queue = dispatch_queue_create("dev.darwinart.network-path", DISPATCH_QUEUE_SERIAL);
  owner.monitor = nw_path_monitor_create();
  if (owner.queue == nullptr || owner.monitor == nullptr) return nullptr;

  __weak DarwinArtNetworkPathMonitor* weak_owner = owner;
  nw_path_monitor_set_update_handler(owner.monitor, ^(nw_path_t path) {
    DarwinArtNetworkPathMonitor* current = weak_owner;
    if (current == nil || current.update == nullptr) return;
    uint32_t flags = 0;
    if (nw_path_is_expensive(path)) flags |= kFlagExpensive;
    if (nw_path_is_constrained(path)) flags |= kFlagConstrained;
    current.update(current.context, PathStatus(path), flags, InterfaceMask(path));
  });
  nw_path_monitor_set_queue(owner.monitor, owner.queue);
  nw_path_monitor_start(owner.monitor);
  return (__bridge_retained void*)owner;
}

extern "C" void darwin_art_network_path_platform_stop(void* handle) {
  if (handle == nullptr) return;
  DarwinArtNetworkPathMonitor* owner = (__bridge_transfer DarwinArtNetworkPathMonitor*)handle;
  nw_path_monitor_cancel(owner.monitor);
  // All update callbacks use this private serial queue. A barrier after cancel
  // drains callback ingress before Rust releases the callback context.
  dispatch_sync(owner.queue, ^{});
  owner.update = nullptr;
  owner.context = nullptr;
  owner.monitor = nil;
  owner.queue = nil;
}

namespace {

constexpr uint32_t kLinkFactsAbiVersion = 1;

bool CopyString(CFTypeRef value, char* output, size_t capacity) {
  if (value == nullptr || CFGetTypeID(value) != CFStringGetTypeID()
      || output == nullptr || capacity == 0) {
    return false;
  }
  return CFStringGetCString(static_cast<CFStringRef>(value), output, capacity,
                            kCFStringEncodingUTF8);
}

bool IsNumericAddress(const char* value) {
  in_addr ipv4{};
  in6_addr ipv6{};
  return inet_pton(AF_INET, value, &ipv4) == 1
      || inet_pton(AF_INET6, value, &ipv6) == 1;
}

}  // namespace

extern "C" int darwin_art_network_link_facts_snapshot(
    DarwinArtNetworkLinkFacts* output) {
  if (output == nullptr) return EINVAL;
  DarwinArtNetworkLinkFacts facts{};
  facts.abi_version = kLinkFactsAbiVersion;
  facts.struct_size = sizeof(facts);

  // State:/Network/Global/IPv4 is the SystemConfiguration-selected primary
  // interface for the machine-wide default path. It is not an Android netId.
  SCDynamicStoreRef store = SCDynamicStoreCreate(
      nullptr, CFSTR("dev.darwinart.network-link-facts"), nullptr, nullptr);
  if (store == nullptr) return EIO;
  CFPropertyListRef ipv4 = SCDynamicStoreCopyValue(
      store, CFSTR("State:/Network/Global/IPv4"));
  if (ipv4 != nullptr && CFGetTypeID(ipv4) == CFDictionaryGetTypeID()) {
    CFTypeRef primary = CFDictionaryGetValue(
        static_cast<CFDictionaryRef>(ipv4), kSCDynamicStorePropNetPrimaryInterface);
    CopyString(primary, facts.interface_name, sizeof(facts.interface_name));
  }
  if (ipv4 != nullptr) CFRelease(ipv4);

  // Read only the global effective DNS list. Per-service DNS (including VPN
  // and scoped resolver domains) remains outside this Android default-path
  // projection and is never merged here.
  CFPropertyListRef dns = SCDynamicStoreCopyValue(
      store, CFSTR("State:/Network/Global/DNS"));
  if (dns != nullptr && CFGetTypeID(dns) == CFDictionaryGetTypeID()) {
    CFTypeRef addresses = CFDictionaryGetValue(
        static_cast<CFDictionaryRef>(dns), kSCPropNetDNSServerAddresses);
    if (addresses != nullptr && CFGetTypeID(addresses) == CFArrayGetTypeID()) {
      CFArrayRef array = static_cast<CFArrayRef>(addresses);
      CFIndex count = CFArrayGetCount(array);
      for (CFIndex i = 0; i < count
          && facts.dns_count < kDarwinArtNetworkMaxDnsServers; ++i) {
        char address[kDarwinArtNetworkDnsAddressCapacity] = {};
        if (!CopyString(CFArrayGetValueAtIndex(array, i), address,
                        sizeof(address)) || !IsNumericAddress(address)) {
          continue;
        }
        std::strncpy(facts.dns_servers[facts.dns_count], address,
                     sizeof(facts.dns_servers[0]) - 1);
        ++facts.dns_count;
      }
    }
  }
  if (dns != nullptr) CFRelease(dns);
  CFRelease(store);
  *output = facts;
  return 0;
}
