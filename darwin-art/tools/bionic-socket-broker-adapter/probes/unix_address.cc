#include "unix_address.h"
#include "unix_endpoints.h"
#include <array>
#include <cassert>
#include <string>
using namespace darwin_art::socket;

int main() {
  std::array<uint8_t, 111> bytes{};
  bytes[0] = 1;
  assert(!ParseUnixAddress(nullptr, 2));
  assert(!ParseUnixAddress(bytes.data(), 1));
  assert(!ParseUnixAddress(bytes.data(), bytes.size()));
  assert(ParseUnixAddress(bytes.data(), 2)->kind == UnixAddressKind::Unnamed);
  const std::string path = "/dev/socket/property_service";
  std::memcpy(bytes.data() + 2, path.data(), path.size());
  auto parsed = ParseUnixAddress(bytes.data(), path.size() + 3);
  assert(parsed && parsed->kind == UnixAddressKind::Pathname);
  assert(std::string(parsed->name.begin(), parsed->name.end()) == path);
  UnixEndpoints endpoints;
  assert(!endpoints.Resolve(*parsed));
  assert(!endpoints.Install(*parsed, "relative.sock"));
  assert(endpoints.Install(*parsed, "/tmp/service-owned/socket"));
  assert(!endpoints.Install(*parsed, "/tmp/replacement"));
  const auto endpoint = endpoints.Resolve(*parsed);
  assert(endpoint && endpoint->sun_family == AF_UNIX);
  assert(std::strcmp(endpoint->sun_path, "/tmp/service-owned/socket") == 0);
  assert(!UnixEndpoints{}.Resolve(*parsed));
  // No implicit read past addrlen, even without a terminating NUL.
  assert(ParseUnixAddress(bytes.data(), 5)->name.size() == 3);
  bytes[2] = 0;
  bytes[3] = 'a'; bytes[4] = 0; bytes[5] = 'b';
  parsed = ParseUnixAddress(bytes.data(), 6);
  assert(parsed && parsed->kind == UnixAddressKind::Abstract);
  assert((parsed->name == std::vector<uint8_t>{'a', 0, 'b'}));
  assert(ParseUnixAddress(bytes.data(), 3)->name.empty());
  bytes[0] = 2;
  assert(!ParseUnixAddress(bytes.data(), 6));
}
