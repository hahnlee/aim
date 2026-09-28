# ADR 0001: Project the macOS default network into Android connectivity

Status: accepted for the current Chromium compatibility slice

## Context

Android 16 applications obtain connectivity through the original
`ConnectivityManager` and `IConnectivityManager`. AIM previously
shadowed those framework classes with a process-local, always-connected Wi-Fi
model. That bypass cannot represent host changes, cost, Binder identity, or
Android callback ordering.

Porting all of `ConnectivityService`, NetworkAgent and netd is outside the
current APK-compatibility goal. Returning canned values from a Binder stub
would merely move the bypass into system_server.

## Decision

- Keep the original Android 16 client classes and AIDL wire contract.
- Publish a system-owned `connectivity` Binder endpoint. Initially it supports
  only contracts exercised by the current unchanged APK; unknown transactions
  remain unsupported.
- Obtain default-path facts from macOS Network.framework on a dedicated native
  queue. Rust owns monitor lifetime and a copied, versioned snapshot; a small
  Darwin shim may translate Network.framework callbacks but owns no Android
  policy.
- Map an active expensive path to Android metered. Map an active non-expensive
  path to unmetered. Initial, unknown, unsatisfied, and requires-connection
  states are conservatively metered, matching Android's behavior when active
  capabilities are absent.
- Preserve macOS constrained/Low Data Mode as a separate fact. It is not
  silently treated as Android metering or Data Saver.
- Do not infer Android `NET_CAPABILITY_VALIDATED` from host reachability and do
  not prevent real socket attempts based on a reachability preflight.
- Compose those host facts inside an Android-owned `ConnectivityServiceState`.
  A dedicated owner `HandlerThread` publishes immutable snapshots and a
  generation-bound `NetworkValidationMonitor` performs probes on a bounded
  worker; application queries never perform validation work.
- For the current Chromium slice, use a pinned HTTP generate-204 probe and
  classify only an exact 204 response as validated. Redirects are captive
  portal observations and all other responses or transport failures remain
  unvalidated. A result for an obsolete host-path generation is discarded.
  This is a deliberately narrow NetworkMonitor-compatible policy slice, not a
  permanent Darwin-specific Internet policy. Full AOSP NetworkStack policy,
  HTTPS/private-DNS probes and captive-portal UI remain the replacement path.
- Add request/callback registration only when an unchanged target APK requires
  it. Android-owned state will then provide request identity, Binder death,
  HandlerThread ordering and the original callback protocol.
- Preserve the original Android 16 `NetworkUtils` and
  `libframework-connectivity-jni.so`. A closed `libandroid.so` multinetwork
  provider implements their public `LIBANDROID` ABI; it is not a replacement
  Java/JNI layer.
- Until the system service publishes real Android Network identities, support
  only `NETWORK_UNSPECIFIED` process/DNS/socket binding. Reject nonzero handles
  instead of silently sending their traffic over the macOS default route.
- Validate socket operations through the central guest-FD owner. Guest FD
  tokens and Android netIds are never interpreted as Darwin descriptors or
  interface indexes.

## Boundary

The Darwin provider exposes fixed-width host facts only: ABI version, snapshot
size, generation, path status, expensive, constrained, IP/DNS support and
interface kinds. Android netIds, `NetworkRequest`, Java/JNI objects and borrowed
Network.framework objects never cross this boundary.

SystemConfiguration may later supply DNS, proxy and interface details for
`LinkProperties`; it does not replace Network.framework for path cost.
Real nonzero network binding will require a system-issued netId-to-live-host-
path registry. At that boundary the Darwin socket provider may use
`IP_BOUND_IF`/`IPV6_BOUND_IF`, while scoped DNS remains a separate provider;
neither changes the machine-wide default route.

## Consequences

This is intentionally less than full Android connectivity-service support, but
it preserves the Android-facing architecture and provides truthful macOS
integration. Unsupported API surface remains visible instead of being reported
as successful. The old `compat/java/android/net` shadows must not be packaged
in the product boot class path.

The current provider is sampled once per second because the fixed-width host
snapshot boundary does not yet expose a wakeup descriptor. This preserves
independent system-server publication but is explicit compatibility debt: a
future native ingress must wake the same Android owner without acquiring
Android policy or Binder authority.
