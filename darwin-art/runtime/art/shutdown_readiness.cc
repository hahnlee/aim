#include "shutdown_readiness.h"

#include "base/locks.h"
#include "base/mutex.h"
#include "runtime.h"
#include "thread-current-inl.h"
#include "thread_list.h"

namespace darwin_art::runtime_art {
bool IsVmReadyForShutdown(art::Thread* owner) {
  art::Runtime* runtime = art::Runtime::Current();
  if (runtime == nullptr || owner == nullptr || art::Thread::Current() != owner)
    return false;
  art::Locks::mutator_lock_->AssertNotHeld(owner);
  // Match pinned ThreadList::WaitForOtherNonDaemonThreadsToExit lock order.
  art::MutexLock shutdown_lock(owner, *art::Locks::runtime_shutdown_lock_);
  if (runtime->IsShuttingDownLocked() || runtime->NumberOfThreadsBeingBorn() != 0)
    return false;
  art::MutexLock threads_lock(owner, *art::Locks::thread_list_lock_);
  if (runtime->GetThreadList()->HasUnregisteringThreads()) return false;
  struct Inspection { art::Thread* owner; bool ready = true; } state{owner};
  runtime->GetThreadList()->ForEach([](art::Thread* thread, void* opaque) {
    auto* inspection = static_cast<Inspection*>(opaque);
    if (thread != inspection->owner && !thread->IsDaemon())
      inspection->ready = false;
  }, &state);
  return state.ready;
}
}
