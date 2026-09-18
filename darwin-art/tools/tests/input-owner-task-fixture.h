#pragma once

// Test-only single-owner provider. It executes the production reusable-task
// queue without linking an AppKit/ALooper provider or fabricating task success.
namespace darwin_art::test {
int DispatchInputOwnerTasks();
}
