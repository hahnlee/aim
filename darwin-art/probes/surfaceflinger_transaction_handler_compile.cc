#include "FrontEnd/TransactionHandler.h"

// This translation unit is compiled separately from the frontend archive. It
// proves that a consumer sees the real AOSP TransactionHandler API and its
// QueuedTransactionState type through the Darwin-generated AIDL/HIDL closure.
extern "C" bool darwin_art_surfaceflinger_frontend_has_pending(
    android::surfaceflinger::frontend::TransactionHandler* handler) {
  return handler != nullptr && handler->hasPendingTransactions();
}
