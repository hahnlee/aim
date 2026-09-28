#pragma once

namespace aim::framework::app {

// Mirrors AndroidRuntime/AppRuntime::onZygoteInit for an application child:
// configure and start original libbinder's process-owned worker pool before
// ActivityThread attaches to ActivityManager.
bool StartApplicationBinderPool();

}  // namespace aim::framework::app
