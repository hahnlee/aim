# Darwin ART profile daemon

`darwin-artd` is the per-profile owner of state shared by Android application
processes. It is deliberately closer to Wine's `wineserver` boundary than to
an app-launch helper: applications request resources and hold leases, while the
daemon alone owns their lifecycle.

## Filesystem ownership

Each profile has one case-sensitive APFS sparse bundle at:

```text
~/Library/Application Support/DarwinART/profiles/<profile>/
  control.sock
  darwin-artd.lock
  android-data.sparsebundle
  mnt/
```

The daemon creates the image as `APFSX`, mounts it without Finder browsing,
verifies that differently-cased names coexist, and initializes Android data,
package, emulated-storage, and run directories. The socket and lock stay on the
host volume so they remain reachable while the guest volume is detached.

The Rust launcher opens a versioned Unix-socket lease and carries its descriptor
through `exec(2)` into each `darwin-art-host`, publishing the real process PID
and package name through `darwin-artctl ps`. The host restores close-on-exec
before it launches children; each Android Service child receives a distinct
lease through the same exec bridge. Shutdown and idle detach are prohibited
while any lease is live. Keeping profile IPC out of the latency-sensitive ART
host link also avoids coupling runtime rebuilds to daemon implementation
changes. A single advisory lock makes one daemon authoritative for a profile,
and peer credentials restrict IPC to the profile owner.

The daemon can also spawn a registered long-lived process with an explicit
argument vector and allowlisted environment. It owns the child handle, records
the real PID/package in `darwin-artctl ps`, holds a profile lease for the whole
lifetime, and reaps the child on exit. This is how `android.system` remains
alive after the application that first caused startup has exited; shell
backgrounding is not part of the lifetime contract. Manager-launched
applications use this same operation, so closing the AppKit process does not
terminate Android applications. Their output is redirected to the selected
profile's `managed-apps.log`.

## macOS application exposure

The AppKit manager projects the installed-package registry into signed `.app`
bundles at `~/Applications/Darwin ART Apps.localized`. These are Chrome-style
application shims, not copied runtimes. Their Info.plist records the Android
package and profile plus the Android label, version, and extracted APK icon. A
small native launcher locates the installed manager bundle, selects the profile,
and invokes the low-level package launcher to resolve the immutable launch
record. That launcher asks `darwin-artd` to daemonize the final app host. The
manager and Finder shims use its asynchronous handoff mode, while direct shell
calls remain synchronous for the requested window duration.

Synchronization rewrites a bundle only when its metadata, icon, launcher, or
manager location changes. Stale-package and deleted-profile cleanup first
validates the `DARManagedAppShim` ownership marker and exact profile, so it can
never remove an unrelated application from the user's Applications directory.

## ART system services

The first persistent framework process is `android.system`, an ART-backed
`system_server-lite` started by `darwin-artd`. It publishes an Android
Binder/Parcel endpoint at `mnt/run/system-server-lite.sock`. The initial
`IPackageRegistry` service resolves immutable package launch records by asking
the daemon, while application-side `PackageManager` uses Binder to resolve
installed packages, explicit Activity and Service components, and
package-scoped launcher intents.

This separation is intentional:

- `darwin-artd` owns macOS authority, filesystem mounts, install records, and
  process handles;
- `android.system` owns Android-observable service APIs and Java object state;
- application/service ART processes consume those APIs over Binder and never
  read another package's registry files directly.

Android Service instances use the existing framework lifecycle bridge. Local
services execute in their application process; declared remote and isolated
services receive distinct ART host processes, Binder channels, and daemon
leases. `onCreate`, `onBind`, asynchronous `ServiceConnection`, rebind,
`onUnbind`, and final process release remain Android-side decisions. Moving
additional ActivityManager policy into `android.system` is an extension of the
same Binder boundary, not a new per-app environment bridge.

## Persistent application data

Each installed package receives private storage below
`mnt/data/apps/<package>/private-data/user/0/<package>`. Guest paths remain
`/data/user/0/<package>`. The Rust filesystem facade validates and translates
only that writable `/data` mount before a host SQLite connection is opened;
immutable mounts cannot be converted into writable host paths. Framework
SharedPreferences uses atomic XML in `shared_prefs`, and SQLite databases plus
journals live in `databases`. Both survive application and daemon relaunch.

The compatibility gate uses unchanged installed AOSP Calculator, Calendar, and
DeskClock packages. Each package must launch twice by package name with no
reinstall, while `darwin-artctl ps` continues to report exactly one
`android.system`. Calculator must retain a valid `Expressions.db` schema;
Calendar and DeskClock must reopen their Android binary-XML (`ABX`) preference
files without a SharedPreferences read error. A cross-package
`PackageManager.getPackageInfo()` request must complete through the system
Binder after the originating app and system process have different PIDs.

## Install and launch

The normal interface separates one-time APK inspection/installation from
package launch:

```sh
cargo xtask build
tools/darwin-art install path/to/app.apk
tools/darwin-art list
tools/darwin-art run com.example.app
tools/darwin-art ps
```

Installation copies the unchanged APK into an immutable, content-addressed
package directory and atomically registers a versioned launch record under the
profile's case-sensitive guest volume. A deoptimized sidecar DEX, when needed,
is copied into the same persistent package-code area. Later `run PACKAGE`
operations resolve that record through `darwin-artd`; they do not inspect the
APK, invoke Cargo, rebuild native code, or reinstall the package. Records and
application data survive daemon detach/restart.

`tools/run-android-apk-app.sh` remains the low-level installer/launcher used by
this interface. Setting `DARWIN_ART_APP_DATA_ROOT` is an explicit test override
that retains the legacy caller-owned storage paths and does not start the
profile daemon.

## Operations

```sh
cargo build --release -p darwin-art-profile --bins
target/release/darwin-artctl ensure
target/release/darwin-artctl status
target/release/darwin-artctl profiles
target/release/darwin-artctl create-profile work
target/release/darwin-artctl profile-size work
target/release/darwin-artctl list
target/release/darwin-artctl ps
target/release/darwin-artctl delete-profile work
target/release/darwin-artctl shutdown
tools/audit-profile-daemon.sh
```

The IPC envelope is versioned (`DARTD001`, protocol version 1). Privileged host
capabilities belong in the daemon; Android-observable shared services belong in
the persistent ART system process and reach applications through Binder.
Profile deletion refuses while an application lease is active, stops the
profile-owned `android.system`, detaches the APFSX volume, and only then removes
the exact validated profile directory.
