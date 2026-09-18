#include "tools/bionic-socket-broker-adapter/src/android_scm_exports.h"

#include <array>
#include <cassert>
#include <cerrno>
#include <cstring>
#include <fcntl.h>
#include <iostream>
#include <unistd.h>

using namespace darwin_art::bionic::scm;

struct Context {
  int source;
  std::array<int, 8> aliases{};
  std::size_t count = 0;
  bool fail_second = false;
};

int Export(void* pointer, int guest) {
  auto& context = *static_cast<Context*>(pointer);
  assert(guest == 17);
  if (context.fail_second && context.count == 1) {
    errno = EMFILE;
    return -1;
  }
  int descriptor = fcntl(context.source, F_DUPFD_CLOEXEC, 0);
  assert(descriptor >= 0);
  context.aliases[context.count++] = descriptor;
  return descriptor;
}

void AssertClosed(const Context& context) {
  for (std::size_t index = 0; index < context.count; ++index) {
    assert(fcntl(context.aliases[index], F_GETFD) == -1 && errno == EBADF);
  }
}

int main() {
  const int source = open("/dev/null", O_RDONLY);
  assert(source >= 0);
  // Unaligned Android header and integer payload, two exports and tail padding.
  std::array<unsigned char, 49> storage{};
  auto* control = storage.data() + 1;
  AndroidControlHeader header{24, 1, 1};
  const int guest[2] = {17, 17};
  std::memcpy(control, &header, sizeof(header));
  std::memcpy(control + sizeof(header), guest, sizeof(guest));
  Context success{source};
  {
    ExportedRights exports;
    assert(exports.Decode(control, 24, Export, &success) == ExportStatus::Ok);
    assert(exports.descriptors().size() == 2);
    for (int alias : exports.descriptors()) assert(fcntl(alias, F_GETFD) >= 0);
    errno = EDOM;
    exports.Clear();
    assert(errno == EDOM && exports.descriptors().empty());
  }
  AssertClosed(success);

  Context failed{source};
  failed.fail_second = true;
  ExportedRights exports;
  assert(exports.Decode(control, 24, Export, &failed) == ExportStatus::DescriptorFailure);
  assert(errno == EMFILE && exports.descriptors().empty());
  AssertClosed(failed);

  // A malformed later record must roll back all previous native exports.
  AndroidControlHeader invalid{15, 1, 1};
  std::memcpy(control + 24, &invalid, sizeof(invalid));
  Context rollback{source};
  assert(exports.Decode(control, 40, Export, &rollback) == ExportStatus::InvalidControl);
  assert(exports.descriptors().empty());
  AssertClosed(rollback);
  header.type = 9;
  std::memcpy(control, &header, sizeof(header));
  assert(exports.Decode(control, 24, Export, &rollback) == ExportStatus::Unsupported);
  assert(exports.Decode(nullptr, 1, Export, &rollback) == ExportStatus::MissingControl);
  assert(exports.Decode(nullptr, 0, Export, &rollback) == ExportStatus::Ok);
  assert(fcntl(source, F_GETFD) >= 0);
  close(source);
  std::cout << "Android SCM exports: unaligned codec/ownership/rollback/errno PASS\n";
}
