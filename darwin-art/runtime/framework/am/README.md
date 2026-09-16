# Application attachment

`ActivityManagerEndpoint` is published by the system process as `activity`.
It handles the original AIDL attach/finish transactions (transaction IDs read
from the loaded AOSP Stub), retaining the callback and sequence for matching
finish notification. Other AMS calls remain unsupported here.

`attachment.cc` verifies kernel Binder caller identity against the Rust profile
registry, loads that package's installed record and maps its ApplicationInfo and
provider metadata. `application_binding.cc` invokes the original
IApplicationThread proxy with a read-only FD duplicated from the system-owned
ApplicationSharedMemory. The sending duplicate is closed after dispatch.

`ActivityManagerClient` routes the legacy bridge's attach/finish calls to that
real proxy. App-side AwaitApplication only creates/attaches ActivityThread and
dispatches its Looper; it no longer calls bindApplication locally. No manual
handleBindApplication, Application constructor or onCreate callback is added.

Remaining: deploy the new profile daemon/system together; verify managed Binder
and FD exchange. This is not full AMS. Death cleanup, launcher-authoritative
startSeq validation, per-process names/configuration, complete package metadata
and remaining provider/activity/service transactions are not complete. The
initial binding currently uses the registered package's main process name and
system Resources configuration. The manager connection is process-lifetime;
reconnection/generation-aware Java endpoints are separate work. No Chrome/UI
acceptance follows from compilation or this module's existence.
