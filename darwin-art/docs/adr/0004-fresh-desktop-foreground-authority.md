# Fresh desktop foreground authority

Status: accepted; WMS activation publication is adopted, first-key correlation remains open.

## Context

Android WMS owns selection and focus publication. The Darwin provider must only
answer whether a captured host process instance is currently foreground.
The system main thread joins the Binder server rather than pumping AppKit.
The pinned macOS SDK's NSRunningApplication documentation states that changing
properties update after main-runloop turns, including reads from other threads.
Using a retained `isActive` object is therefore not fresh host authority here.

## Decision

Use public synchronous `GetFrontProcess` / `GetProcessPID` in one narrow provider.
Although deprecated since macOS 10.9, these remain available for arm64 in the
pinned SDK. Suppress deprecation only at the calls, not across runtime sources.
Capture PID plus kernel process birth via `proc_pidinfo(PROC_PIDTBSDINFO)` during
authenticated server-side root registration. A foreground query checks that
original birth before and after the host query and rejects errors or mismatch.
Clients cannot supply birth identity. This preserves the guest-policy/real-host
provider boundary used in Wine-style integration; no private API or activation
side effect is used.

## Limits and migration

A sample is not an atomic lease on host foreground state or a global transition
sequence. WMS must serialize reconciliation, revalidate before granting focus,
and correlate root event serial, selected original channel and epoch with receiver
readiness. Local resign/close must revoke pending key admission immediately.
This provider alone does not establish first-key delivery.
When the system has a genuinely dispatched AppKit authority actor, replace the
deprecated query behind the same interface with a verified fresh modern provider;
do not change Android policy or rely on unverified cached properties.
