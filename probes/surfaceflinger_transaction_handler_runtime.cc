#include "FrontEnd/TransactionHandler.h"
#include "transaction_bridge.h"

#include <array>
#include <cstdio>

int main() {
  using android::QueuedTransactionState;
  using android::surfaceflinger::frontend::TransactionHandler;

  TransactionHandler handler;
  if (handler.hasPendingTransactions()) {
    return 10;
  }

  QueuedTransactionState transaction{};
  transaction.flags = 0;
  transaction.desiredPresentTime = 0;
  transaction.isAutoTimestamp = true;
  transaction.postTime = 0;
  transaction.hasListenerCallbacks = false;
  transaction.originPid = 1;
  transaction.originUid = 1;
  transaction.id = 1;
  handler.queueTransaction(std::move(transaction));
  if (!handler.hasPendingTransactions()) {
    return 11;
  }

  handler.collectTransactions();
  auto flushed = handler.flushTransactions();
  if (flushed.size() != 1 || flushed.front().id != 1 ||
      handler.hasPendingTransactions()) {
    return 12;
  }

  const AimSurfaceFlingerLayerUpdate updates[] = {
      {.layer_id = 41,
       .parent_id = 0,
       .what = android::layer_state_t::ePositionChanged |
               android::layer_state_t::eAlphaChanged,
       .x = 12.0f,
       .y = 24.0f,
       .z = 3,
       .alpha = 1.0f,
       .destination_left = 0,
       .destination_top = 0,
       .destination_right = 360,
       .destination_bottom = 640},
      {.layer_id = 42,
       .parent_id = 41,
       .what = android::layer_state_t::eLayerChanged |
               android::layer_state_t::eDestinationFrameChanged,
       .x = 0.0f,
       .y = 0.0f,
       .z = 4,
       .alpha = 0.8f,
       .destination_left = 10,
       .destination_top = 20,
       .destination_right = 350,
       .destination_bottom = 620},
  };
  AimSurfaceFlingerCommitResult result{};
  if (!aim_surfaceflinger_commit_transaction(
          2, updates, std::size(updates), &result) ||
      result.transaction_id != 2 || result.transaction_count != 1 ||
      result.layer_state_count != std::size(updates)) {
    return 13;
  }

  size_t order_count = 0;
  if (!aim_surfaceflinger_copy_layer_order(nullptr, 0, &order_count) ||
      order_count != 2) {
    std::fprintf(stderr,
                 "surfaceflinger-transaction-runtime: initial order count=%zu "
                 "expected=2\n",
                 order_count);
    return 14;
  }
  std::array<uint32_t, 2> order{};
  if (!aim_surfaceflinger_copy_layer_order(order.data(), order.size(),
                                                   &order_count) ||
      order != std::array<uint32_t, 2>{41, 42}) {
    return 15;
  }

  const AimSurfaceFlingerLayerUpdate move_child_below[] = {
      {.layer_id = 42,
       .parent_id = 41,
       .what = android::layer_state_t::eLayerChanged,
       .x = 0.0f,
       .y = 0.0f,
       .z = -4,
       .alpha = 0.8f,
       .destination_left = 10,
       .destination_top = 20,
       .destination_right = 350,
       .destination_bottom = 620},
  };
  if (!aim_surfaceflinger_commit_transaction(
          3, move_child_below, std::size(move_child_below), &result) ||
      !aim_surfaceflinger_copy_layer_order(order.data(), order.size(),
                                                   &order_count) ||
      order != std::array<uint32_t, 2>{42, 41}) {
    return 16;
  }
  const std::array<uint32_t, 2> destroyed{41, 42};
  if (!aim_surfaceflinger_destroy_layer_handles(destroyed.data(),
                                                        destroyed.size()) ||
      !aim_surfaceflinger_copy_layer_order(nullptr, 0, &order_count) ||
      order_count != 0) {
    return 17;
  }

  // A buffer-producing child can arrive without its structural parent's own
  // buffer state. The central lifecycle must still mirror the parent handle
  // before attaching the child, as SurfaceFlinger does independently of
  // whether a layer currently has BufferState.
  const AimSurfaceFlingerLayerUpdate child_only[] = {
      {.layer_id = 52,
       .parent_id = 51,
       .what = android::layer_state_t::eBufferChanged,
       .x = 0.0f,
       .y = 0.0f,
       .z = 1,
       .alpha = 1.0f,
       .destination_left = 0,
       .destination_top = 0,
       .destination_right = 360,
       .destination_bottom = 640},
  };
  if (!aim_surfaceflinger_commit_transaction(
          4, child_only, std::size(child_only), &result) ||
      !aim_surfaceflinger_copy_layer_order(nullptr, 0, &order_count) ||
      order_count != 2) {
    return 18;
  }
  std::array<uint32_t, 2> child_only_order{};
  if (!aim_surfaceflinger_copy_layer_order(
          child_only_order.data(), child_only_order.size(), &order_count) ||
      child_only_order != std::array<uint32_t, 2>{51, 52}) {
    return 19;
  }
  const std::array<uint32_t, 2> child_only_destroyed{51, 52};
  if (!aim_surfaceflinger_destroy_layer_handles(
          child_only_destroyed.data(), child_only_destroyed.size()) ||
      !aim_surfaceflinger_copy_layer_order(nullptr, 0, &order_count) ||
      order_count != 0) {
    return 20;
  }

  // SurfaceView uses relative Z to place its buffer subtree below ViewRoot
  // while keeping both controls in the same display hierarchy.
  const AimSurfaceFlingerLayerUpdate relative_roots[] = {
      {.layer_id = 61,
       .parent_id = 0,
       .what = android::layer_state_t::eLayerChanged,
       .z = 0,
       .alpha = 1.0f},
      {.layer_id = 62,
       .parent_id = 0,
       .relative_parent_id = 61,
       .what = android::layer_state_t::eRelativeLayerChanged,
       .z = -2,
       .alpha = 1.0f},
  };
  if (!aim_surfaceflinger_commit_transaction(
          5, relative_roots, std::size(relative_roots), &result) ||
      !aim_surfaceflinger_copy_layer_order(nullptr, 0, &order_count) ||
      order_count != 2) {
    return 21;
  }
  std::array<uint32_t, 2> relative_order{};
  if (!aim_surfaceflinger_copy_layer_order(
          relative_order.data(), relative_order.size(), &order_count) ||
      relative_order != std::array<uint32_t, 2>{62, 61}) {
    return 22;
  }
  const std::array<uint32_t, 2> relative_destroyed{61, 62};
  if (!aim_surfaceflinger_destroy_layer_handles(
          relative_destroyed.data(), relative_destroyed.size())) {
    return 23;
  }

  // The pointer-free Darwin bridge cannot populate Binder parent handles.
  // A pure eReparent(root -> null) must nevertheless invalidate the cached
  // hierarchy; otherwise a root that has become detached remains in the
  // display-root order.
  const AimSurfaceFlingerLayerUpdate detach_case[] = {
      {.layer_id = 71,
       .parent_id = 0,
       .what = android::layer_state_t::eLayerChanged,
       .z = 1,
       .alpha = 1.0f},
      {.layer_id = 73,
       .parent_id = 0,
       .what = android::layer_state_t::eLayerChanged,
       .z = 3,
       .alpha = 1.0f},
      {.layer_id = 72,
       .parent_id = 0,
       .what = android::layer_state_t::eLayerChanged,
       .z = 2,
       .alpha = 1.0f},
  };
  if (!aim_surfaceflinger_commit_transaction(
          6, detach_case, std::size(detach_case), &result) ||
      !aim_surfaceflinger_copy_layer_order(nullptr, 0, &order_count) ||
      order_count != 3) {
    return 24;
  }
  std::array<uint32_t, 3> detach_order{};
  if (!aim_surfaceflinger_copy_layer_order(
          detach_order.data(), detach_order.size(), &order_count) ||
      detach_order != std::array<uint32_t, 3>{71, 72, 73}) {
    return 25;
  }
  const AimSurfaceFlingerLayerUpdate detach_child[] = {
      {.layer_id = 72,
       .parent_id = 0,
       .what = android::layer_state_t::eReparent,
       .z = 2,
       .alpha = 1.0f},
  };
  if (!aim_surfaceflinger_commit_transaction(
          7, detach_child, std::size(detach_child), &result) ||
      !aim_surfaceflinger_copy_layer_order(
          detach_order.data(), detach_order.size(), &order_count) ||
      order_count != 2 ||
      std::array<uint32_t, 3>{detach_order[0], detach_order[1], 0} !=
          std::array<uint32_t, 3>{71, 73, 0}) {
    return 26;
  }
  // A repeated null-parent transaction is a no-op after the first detach and
  // must not perturb the surviving root order.
  if (!aim_surfaceflinger_commit_transaction(
          8, detach_child, std::size(detach_child), &result) ||
      !aim_surfaceflinger_copy_layer_order(
          detach_order.data(), detach_order.size(), &order_count) ||
      order_count != 2 || detach_order[0] != 71 || detach_order[1] != 73) {
    return 27;
  }
  const std::array<uint32_t, 3> detach_destroyed{71, 72, 73};
  if (!aim_surfaceflinger_destroy_layer_handles(
          detach_destroyed.data(), detach_destroyed.size()) ||
      !aim_surfaceflinger_copy_layer_order(nullptr, 0, &order_count) ||
      order_count != 0) {
    return 28;
  }

  std::puts(
      "surfaceflinger-transaction-runtime: queue=1 flush=1 pending=0 "
      "resolved-layers=2 hierarchy-order=PASS lifecycle=PASS "
      "buffer-child-parent=PASS relative-layer=PASS detach-reparent=PASS");
  return 0;
}
