#include "../compat/binder/calling_identity.h"
#include <cassert>
#include <thread>
#include <unistd.h>
#include <cstdio>
using namespace darwin_art::binder;
// Test seam for the separately socket-tested Rust registry client.
extern "C" int32_t darwin_art_runtime_registered_process_uid(uint32_t) { return 10001; }
int main() {
  assert(CurrentIdentity().pid == getpid());
  {
    IncomingIdentity caller(12345, 10042);
    assert(CurrentIdentity().pid == 12345 && CurrentIdentity().uid == 10042);
    int64_t token = ClearIdentity();
    assert(CurrentIdentity().pid == getpid() && CurrentIdentity().explicit_identity);
    assert(CurrentIdentity().uid == 10001);
    int64_t inner = ClearIdentity();
    RestoreIdentity(inner);
    assert(CurrentIdentity().explicit_identity);
    RestoreIdentity(token);
    assert(CurrentIdentity().pid == 12345 && CurrentIdentity().uid == 10042);
    assert(!CurrentIdentity().explicit_identity);
    {
      IncomingIdentity nested(0, -1);
      token = ClearIdentity();
      RestoreIdentity(token);
      assert(CurrentIdentity().uid == -1 && CurrentIdentity().pid == 0);
    }
    assert(CurrentIdentity().pid == 12345);
    std::thread thread([] { assert(CurrentIdentity().pid == getpid()); });
    thread.join();
  }
  assert(CurrentIdentity().pid == getpid() && !CurrentIdentity().remote);
  { IncomingIdentity caller(-1, -1); auto token = ClearIdentity(); RestoreIdentity(token);
    assert(CurrentIdentity().pid == -1 && CurrentIdentity().uid == -1); }
  puts("Binder identity nesting/clear/restore/TLS PASS");
}
