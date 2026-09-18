#pragma once

#include "android_looper_owner.h"

#include <memory>

namespace darwin_art::looper::detail {

struct ReusableTaskQueue;

// Owner bridge: ALooper owns the queue object and its process-lifetime
// association; this module owns all task state and queue transitions.
ReusableTaskQueue* ReusableTaskQueueForLooper(void* looper);
ReusableTaskQueue* CreateReusableTaskQueue();
void DestroyReusableTaskQueue(ReusableTaskQueue* queue);

std::shared_ptr<ReusableLooperTask::State> PrepareReusableTaskState(
    void* looper, const ReusableLooperTaskCallbacks& callbacks);
bool RequestReusableTask(const std::shared_ptr<ReusableLooperTask::State>& state);
bool CancelReusableTask(const std::shared_ptr<ReusableLooperTask::State>& state);
bool IsReusableTaskQuiescent(
    const std::shared_ptr<ReusableLooperTask::State>& state);
int DispatchReusableTasks(ReusableTaskQueue* queue);

}  // namespace darwin_art::looper::detail
