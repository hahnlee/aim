#include "compat/binder/wire_dispatch_policy.h"

#include <cassert>

int main() {
  using darwin_art::binder::DecideWireDispatch;

  const auto handled = DecideWireDispatch(true, true);
  assert(handled.response_status == 0);
  assert(handled.keep_channel);

  const auto unknown_transaction = DecideWireDispatch(false, true);
  assert(unknown_transaction.response_status == -1);
  assert(unknown_transaction.keep_channel);

  const auto transport_failure = DecideWireDispatch(true, false);
  assert(transport_failure.response_status == 0);
  assert(!transport_failure.keep_channel);
}
