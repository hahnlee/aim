// Links the production darwin_android_platform.mm object. Empty transactions
// exercise callback ownership without a display/GPU fixture; presentation is
// deliberately not tested here. Other provider symbols remain lazy imports.
#include "../compat/darwin_android_platform.h"

#include <cassert>
#include <cstdio>
#include <cstdlib>
#include <cerrno>
#include <unistd.h>

struct ASurfaceTransaction;
struct ASurfaceTransactionStats;
struct ASurfaceControl;
struct AHardwareBuffer;
extern "C" void ASurfaceControl_release(ASurfaceControl*);
extern "C" void ASurfaceTransaction_setBuffer(
    ASurfaceTransaction*, ASurfaceControl*, AHardwareBuffer*, int);
// Test descriptor transport uses ordinary pipes; transaction ownership is the
// production object, while broker namespace translation is tested separately.
static bool fail_fence = false;
extern "C" int aim_bionic_socket_broker_dup(int fd) {
  if (fail_fence) { errno = EMFILE; return -1; }
  return dup(fd);
}
extern "C" int aim_bionic_socket_broker_close(int fd) { return close(fd); }
extern "C" int sync_wait(int, int) { errno = EIO; return -1; }
extern "C" ASurfaceTransaction* ASurfaceTransaction_create();
extern "C" void ASurfaceTransaction_delete(ASurfaceTransaction*);
extern "C" void ASurfaceTransaction_apply(ASurfaceTransaction*);
extern "C" void ASurfaceTransaction_setOnCommit(
    ASurfaceTransaction*, void*, void (*)(void*, ASurfaceTransactionStats*));
extern "C" void ASurfaceTransaction_setOnComplete(
    ASurfaceTransaction*, void*, void (*)(void*, ASurfaceTransactionStats*));
extern "C" void aim_android_set_hardware_buffer_composition_active(bool) {}

struct Counts { int commit = 0; int complete = 0; int discard = 0; };
void Commit(void* value, ASurfaceTransactionStats*) {
  ++static_cast<Counts*>(value)->commit;
}
void Complete(void* value, ASurfaceTransactionStats*) {
  ++static_cast<Counts*>(value)->complete;
}
void Discard(void* value) { ++static_cast<Counts*>(value)->discard; }
void DiscardBuffer(void* value, int fence) {
  ++static_cast<Counts*>(value)->discard;
  if (fail_fence) { assert(fence == -2); return; }
  assert(fence >= 0);
  char byte = 0;
  assert(read(fence, &byte, 1) == 1 && byte == 'x');
  close(fence);
}
void AddBuffer(ASurfaceTransaction* transaction, ASurfaceControl* control,
               Counts& counts) {
  int descriptors[2];
  assert(pipe(descriptors) == 0);
  assert(write(descriptors[1], "x", 1) == 1);
  close(descriptors[1]);
  ASurfaceTransaction_setBuffer(transaction, control, nullptr, descriptors[0]);
  aim_android_surface_transaction_set_buffer_callbacks(
      transaction, control, &counts, Complete, DiscardBuffer);
}
ASurfaceTransaction* Make(Counts& counts) {
  auto* transaction = ASurfaceTransaction_create();
  assert(transaction != nullptr);
  ASurfaceTransaction_setOnCommit(transaction, &counts, Commit);
  ASurfaceTransaction_setOnComplete(transaction, &counts, Complete);
  aim_android_surface_transaction_set_on_discard(
      transaction, &counts, Discard);
  return transaction;
}
void CloseOnCommit(void* transaction, ASurfaceTransactionStats*) {
  ASurfaceTransaction_delete(static_cast<ASurfaceTransaction*>(transaction));
}
struct ReuseContext { ASurfaceTransaction* transaction; Counts next; };
void ReuseOnCommit(void* value, ASurfaceTransactionStats*) {
  auto* context = static_cast<ReuseContext*>(value);
  ASurfaceTransaction_setOnComplete(context->transaction, &context->next, Complete);
}

int main() {
  // No layers means no service request. This selects the normal transaction
  // callback boundary while keeping this ownership test display-independent.
  setenv("AIM_SURFACEFLINGER_SOCKET", "callback-smoke", 1);
  Counts a, b;
  auto* destination = Make(a);
  auto* source = Make(b);
  aim_android_surface_transaction_merge(destination, source);
  ASurfaceTransaction_delete(source);
  assert(a.discard == 0 && b.discard == 0);
  ASurfaceTransaction_apply(destination);
  assert(a.commit == 1 && b.commit == 1);
  assert(a.complete == 1 && b.complete == 1);
  assert(a.discard == 0 && b.discard == 0);
  ASurfaceTransaction_apply(destination);
  ASurfaceTransaction_delete(destination);
  assert(a.commit == 1 && b.commit == 1);
  assert(a.complete == 1 && b.complete == 1);
  assert(a.discard == 0 && b.discard == 0);

  Counts c, d;
  destination = Make(c);
  source = Make(d);
  aim_android_surface_transaction_merge(destination, source);
  ASurfaceTransaction_delete(source);
  aim_android_surface_transaction_clear(destination);
  ASurfaceTransaction_apply(destination);
  ASurfaceTransaction_delete(destination);
  assert(c.discard == 1 && d.discard == 1);
  assert(c.commit == 0 && d.commit == 0);
  assert(c.complete == 0 && d.complete == 0);
  auto* control = static_cast<ASurfaceControl*>(
      aim_android_surface_control_create_root("callback-test"));
  Counts e, f, g;
  destination = ASurfaceTransaction_create();
  source = ASurfaceTransaction_create();
  AddBuffer(destination, control, e);
  AddBuffer(source, control, f);
  aim_android_surface_transaction_merge(destination, source);
  assert(e.discard == 1 && f.discard == 0);
  ASurfaceTransaction_delete(source);
  assert(f.discard == 0);
  AddBuffer(destination, control, g);
  assert(f.discard == 1 && g.discard == 0);
  ASurfaceTransaction_delete(destination);
  assert(g.discard == 1 && e.complete == 0 && f.complete == 0);
  ASurfaceControl_release(control);
  control = static_cast<ASurfaceControl*>(
      aim_android_surface_control_create_root("quarantine-test"));
  Counts quarantine;
  destination = ASurfaceTransaction_create();
  AddBuffer(destination, control, quarantine);
  fail_fence = true;
  ASurfaceTransaction_delete(destination);
  fail_fence = false;
  assert(quarantine.discard == 1);
  ASurfaceControl_release(control);
  Counts closed;
  destination = ASurfaceTransaction_create();
  ASurfaceTransaction_setOnCommit(destination, destination, CloseOnCommit);
  ASurfaceTransaction_setOnComplete(destination, &closed, Complete);
  ASurfaceTransaction_apply(destination);
  assert(closed.complete == 1);
  ReuseContext reuse{ASurfaceTransaction_create(), {}};
  ASurfaceTransaction_setOnCommit(reuse.transaction, &reuse, ReuseOnCommit);
  ASurfaceTransaction_apply(reuse.transaction);
  assert(reuse.next.complete == 0);
  ASurfaceTransaction_apply(reuse.transaction);
  assert(reuse.next.complete == 1);
  ASurfaceTransaction_delete(reuse.transaction);
  std::puts("surface-transaction-callback-smoke: PASS merge/apply/clear/delete and replaced-buffer fence ownership");
}
