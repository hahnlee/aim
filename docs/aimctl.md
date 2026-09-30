# aimctl: the resident guest and its apps (#473)

`aimctl` (`crates/aimctl`) is the user's manager of a guest that stays up
in the background, one per data directory, and of its apps. It is built on
what the boot already uses: guest-init and its data image
([storage.md](storage.md)), aim-display, the per-app shims of aim-apps in
window mode ([windows.md](windows.md)), and linux-run joining the guest's
pid namespace. Apps are installed, opened and removed by Android's own
`pm` and `am`, run in the guest.

```sh
aimctl start [--windows]      # the guest in the background
aimctl status                 # boot state, uptime, CPU, memory, data image
aimctl shell [COMMAND...]     # a shell in the guest, or one command
aimctl apps                   # the launcher apps: package and label
aimctl install APK [SPLIT...] # pm install -r of copies of the APKs
aimctl uninstall PACKAGE      # pm uninstall
aimctl open PACKAGE           # the app, in its own window in window mode
aimctl logs [--follow] [LOGCAT OPTION...]
aimctl stop
aimctl run [--windows]        # what start runs: the guest in the foreground
```

Every command takes `--data DIR`, the data directory; the default is
`~/Library/Application Support/aim/data`. Tests and checks use a
disposable one. `aimctl` runs the programs beside it (`guest-init`,
`aim-display`, `aim-apps`, `linux-run`, as `cargo aim build` puts them in
`target/release`) on the derived image, ANGLE and MoltenVK that `cargo aim
build` built (docs/build.md).

## The resident guest

`aimctl run` is the guest's keeper: it starts aim-display (a device window
of 1080×1920, or window mode with `--windows`), in window mode
`aim-apps shims --watch` (a shim per launcher app, following installs), and
guest-init on the data directory, and stops them when guest-init ends or
it gets SIGTERM, SIGINT or SIGHUP: guest-init stops its services and
detaches the data image (three minutes at most, then it is killed), then
the display server and the shims' keeper get SIGTERM. The shims quit with
the display server.

`aimctl start` runs `aimctl run` in a session of its own, with its output
in the log, and returns once guest-init has laid out the guest (the first
start creates the data image). `aimctl status` then says `booting`, and
`running` once Android set `sys.boot_completed`. `aimctl stop` sends the
keeper SIGTERM and waits until it is gone.

Beside the data directory `DATA` (its image `DATA.asif`, guest-init's
`DATA.lock`) lies `DATA.aimctl/`:

| File | What |
| --- | --- |
| `lock` | held (`flock`) by `aimctl run` while the guest is resident |
| `state` | the keeper's pid, guest-init's pid (its binder is `dev.aim.guest-init.<pid>.binder`), the mode and the start time |
| `log` | the output of the keeper, guest-init, aim-display and aim-apps |
| `display`, `capture.bmp` | aim-display's socket and captures (SIGUSR1) |
| `apps/` | the shims (window mode), registered with Launch Services |

**One guest per data directory.** A guest is resident while the keeper
holds `lock`; a `state` left by a keeper that was killed is not one.
`start` and `run` refuse a data directory with a resident guest, and one
whose data image another guest-init holds (`DATA.lock`, as `cargo aim
boot` on it does).

## Talking to the guest

`shell`, `logs`, `install`, `uninstall`, `open` and `status` run guest
programs with `linux-run --root <derived> --path-map DATA/run/path-map
--binder dev.aim.guest-init.<pid>.binder --by-pid DATA/run/identity/by-pid`,
so they see the guest's files, reach its services, and see and signal its
processes (`ps -A`, `kill`), as `adb shell` does on a device.

- **install** copies the APKs (APFS clones) into the guest's
  `/data/local/tmp`, runs `pm install -r` on them (several APKs are one
  app with its splits) and removes the copies; the APKs themselves are
  never changed. Runtime permissions are not granted: the app asks for
  them as on a phone. In window mode the shim of a new launcher app
  appears within the keeper's poll (two seconds), and an uninstalled
  app's shim goes.
- **apps** lists the launcher apps as aim-apps finds them: the image's
  and `/data/app`'s APKs with an enabled MAIN/LAUNCHER activity, less the
  packages and components disabled for user 0.
- **open** in window mode opens the app's shim (`open`), which launches
  the app in a new task and hosts its windows under its own Dock icon; in
  device mode, or before the shim exists, it runs `am start -W` of the
  launcher activity.
- **logs** is `logcat -d`, or `logcat` with `--follow`; other options go
  to logcat (`aimctl logs -b events`, `aimctl logs -s ActivityManager`).
- **status** measures guest-init and the linux-run processes on the
  guest's path map (`ps`: resident memory, recent CPU in percent of a
  core) and the data image (what its file occupies, and what the guest's
  files use while it is attached).

`install`, `uninstall` and `open` wait for `sys.boot_completed`; the
framework calls have time limits (ten minutes for `pm install`, two for
the others).
