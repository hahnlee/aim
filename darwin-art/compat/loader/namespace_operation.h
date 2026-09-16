#pragma once
#include "darwin_art_linker_namespace.h"
#include <cassert>
namespace darwin_art::loader {
// Stack-bound guard; native token enforces same-thread recursive ownership.
class NamespaceOperation {
 public:
  explicit NamespaceOperation(LinkerRegistry* registry)
      : token_(darwin_art_linker_operation_enter(registry)) {}
  ~NamespaceOperation() {
    if (token_) {
      const int status = darwin_art_linker_operation_leave(token_);
      assert(status == 0);
      (void)status;
    }
  }
  NamespaceOperation(const NamespaceOperation&) = delete;
  NamespaceOperation& operator=(const NamespaceOperation&) = delete;
  explicit operator bool() const { return token_ != nullptr; }
 private:
  LinkerOperation* token_;
};
}
