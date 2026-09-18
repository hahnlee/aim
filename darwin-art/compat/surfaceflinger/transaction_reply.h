#pragma once

#include <memory>

namespace darwin_art::surfaceflinger {

// Narrow host transport owner. Queue admission never sends a commit receipt.
// The Android transaction owner calls Committed only after real acceptance.
// Closing an unresolved reply produces EOF (unknown), not a safe rejection.
// Destruction is close-only, so queue abort can release it under its lock.
class TransactionReply final {
 public:
  static std::shared_ptr<TransactionReply> Create(int client, int completion);
  ~TransactionReply();
  TransactionReply(const TransactionReply&) = delete;
  TransactionReply& operator=(const TransactionReply&) = delete;
  bool Committed() noexcept;
  bool Rejected(int status) noexcept;

 private:
  TransactionReply(int client, int completion)
      : client_(client), completion_(completion) {}
  bool Send(bool committed, int status) noexcept;
  int client_;
  int completion_;
};

}  // namespace darwin_art::surfaceflinger
