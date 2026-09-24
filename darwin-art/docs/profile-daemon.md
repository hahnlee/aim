# AIM profile daemon

`darwin-artd` is the per-profile owner of shared host state. Applications request
resources and retain leases; the daemon owns mounts, install records and managed
process lifetimes. Android-observable policy belongs in `android.system`.

## Profile and filesystem

Each profile uses a case-sensitive APFS sparse bundle:

```text
~/Library/Application Support/DarwinART/profiles/<profile>/
  control.sock
  darwin-artd.lock
  android-data.sparsebundle
  mnt/
```

The daemon creates `APFSX`, verifies case sensitivity and initializes Android
data, packages, emulated storage and runtime directories. Socket and lock remain
on the host volume. One advisory lock selects the daemon; peer credentials limit
IPC to the profile owner.

Every launched host inherits a versioned Unix-socket lease and publishes its real
PID/package through `darwin-artctl ps`. Service children receive distinct leases.
Live leases prevent detach and shutdown. The daemon owns managed child handles,
reaps exits and redirects output to `managed-apps.log`; shell backgrounding is
not a lifetime mechanism.

## Host and Android ownership

- `darwin-artd`: mounts, immutable install records, process handles and other
  macOS capabilities.
- `android.system`: ART-backed shared Android Binder services and Java state.
- app/service ART processes: consume Binder APIs and never read another
  package's registry files directly.

`android.system` remains alive independently of the app that triggered startup
and publishes its Binder endpoint below `mnt/run`. Local, remote and isolated
Android services retain framework lifecycle ownership; the daemon only provides
the process and lease mechanism.

Each package's writable state lives under
`mnt/data/apps/<package>/private-data/user/0/<package>` and appears as
`/data/user/0/<package>`. The filesystem facade translates only the authorized
writable mount. SharedPreferences, SQLite databases and journals survive app and
daemon relaunch.

## Installed applications

The AppKit manager projects installed records into signed shims at
`~/Applications/Darwin ART Apps.localized`. A shim stores package/profile and
display metadata, then asks the manager and daemon to launch the immutable
record. It contains no copied runtime. Synchronization rewrites changed shims
only; deletion validates the ownership marker and exact profile.

Normal usage separates installation from launch:

```sh
cargo xtask build
tools/darwin-art install path/to/app.apk
tools/darwin-art list
tools/darwin-art run com.example.app
tools/darwin-art ps
```

Install copies the unchanged APK into a content-addressed directory and
atomically registers a versioned record. A required deoptimized DEX is persisted
with package code. `run PACKAGE` resolves that record; it does not inspect or
reinstall the APK, invoke Cargo or rebuild native code.

`tools/run-android-apk-app.sh` remains the low-level installer/launcher.
`DARWIN_ART_APP_DATA_ROOT` is a test-only caller-owned storage override and does
not start the profile daemon.

## Acceptance and operations

The unchanged Calculator, Calendar and DeskClock gate launches every package
twice without reinstall and retains exactly one `android.system`. It verifies
Calculator SQLite, Calendar/DeskClock binary preferences and cross-process
PackageManager lookup. Run the daemon audit after lifecycle, filesystem, install
or service-process changes.

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

IPC uses envelope `DARTD001`, protocol version 1. Profile deletion refuses live
leases, stops the profile-owned system process, detaches APFSX and removes only
the validated profile directory.
