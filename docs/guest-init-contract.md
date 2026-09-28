# Guest init ↔ syscall layer contract

This is what `crates/aim-guest-init` (aimd's init role, ADR 0012
item 6) provides to each service process, and what `crates/aim-linux-abi`
(`linux-run`) must do with it. guest-init owns everything init owns: the
property areas and property service, service processes, their descriptors and
credentials, and the writable parts of the filesystem. The syscall layer
presents that state to the guest through Linux syscalls.

Tracking: #153 (migration), #173 (the `linux-run` side of this contract),
#174 (syscalls the first real service launches need).

## 1. Process launch

Each service is one host process, started in a new process group with
`posix_spawn`:

```
linux-run --root <image> --path-map <runtime>/path-map \
          --identity <runtime>/identity/<service>.<n> --inherit-env \
          --binder <name> [--seclabel <label>] [--gpu <dir>] \
          [--vulkan <dir>] [--display <socket>] [--trace] <program> <args...>
```

- `<program>` and `<args>` are the guest argv after init's `${prop}`
  expansion. `argv[0]` is the program path, as init passes it.
- `--binder` names the binder host: guest-init embeds
  `aim-binder-host`'s server (`Server::start`) under the bootstrap name
  `dev.aim.guest-init.<pid>.binder`, one per boot, since the driver is
  kernel state (ADR 0012 item 7). The layer serves `/dev/binder`,
  `/dev/hwbinder`, `/dev/vndbinder` and their binderfs nodes
  `/dev/binderfs/<device>`, which init's `symlink`s point the former at.
- `--seclabel` is the service's `seclabel`. Without one, init would compute
  the domain from the executable's file context; guest-init does not yet,
  and the layer reports `u:r:init:s0`.
- `--gpu`, `--vulkan` and `--display` are host devices every service may
  use, given to guest-init with the same options: the host's ANGLE behind
  the GLES driver (`docs/gles-driver.md`), MoltenVK behind the Vulkan
  driver (`docs/vulkan-driver.md`) and the display server behind the
  composer (`docs/composer.md`).
- stdin, stdout and stderr are `/dev/null`, as init gives a service
  without `stdio_to_kmsg` (zygote refuses to fork while it holds any other
  file). guest-init opens `<runtime>/logs/<service>.log` as stderr and
  passes `--stdio-null`: `linux-run` keeps that stderr, hidden from the
  guest and across its execs (`--diag-fd`), for the layer's own messages,
  then puts `/dev/null` in fds 0-2.
- Only the service's descriptors are inherited
  (`POSIX_SPAWN_CLOEXEC_DEFAULT`); see section 5.
- guest-init stops a service with `kill(-pgid, SIGKILL)` (`SIGTERM` for
  `gentle_kill`), the same as init's `KillProcessGroup`. Processes the guest
  forks must stay in that group.
- guest-init reads the exit with `waitpid`. The syscall layer must report the
  guest's exit the way Linux would. `exit_group(n)` must become host exit
  status `n`. A guest killed by a signal must end the host process with that
  signal (reset it to `SIG_DFL` and re-raise), not with `_exit(128 + sig)`.
  init's crash accounting (`critical`, `reboot_on_failure`, `onrestart`)
  depends on telling the two apart.

`linux-run` must list the new options in its usage text: guest-init runs
`linux-run --help` and passes only the options it finds there. Without them
it logs that services will see the image's own `/dev`, the host uid, no
binder and `default_android_env()`.

## 2. `--path-map FILE`: the guest filesystem view

The file is UTF-8 text, one mapping per line, tab-separated. Lines starting
with `#` are comments.

```
# aim-guest-init path map v1
root	/	<image root>
rw	/dev	<runtime>/dev
rw	/apex/apex-info-list.xml	<runtime>/apex/apex-info-list.xml
kernfs	/proc	<runtime>/kernfs/proc
...
```

| Guest | Host | Kind | Lifetime |
| --- | --- | --- | --- |
| `/dev` | `<runtime>/dev` | `rw` | per boot (tmpfs on a device) |
| `/mnt`, `/tmp`, `/storage`, `/config`, `/data_mirror`, `/linkerconfig` | `<runtime>/<name>` | `rw` | per boot |
| `/apex/apex-info-list.xml` | `<runtime>/apex/apex-info-list.xml` | `rw` (one file) | per boot |
| `/data`, `/metadata`, `/cache` | `<data>/<name>` | `rw` | persistent |
| `/data/user/0` | `<data>/data/data` | `rw` | persistent (vold's bind of `/data/data`) |
| `/proc`, `/sys` | `<runtime>/kernfs/{proc,sys}` | `kernfs` | per boot |
| `/sys/fs/cgroup` | `<runtime>/cgroup` | `cgroup2` | per boot |
| `/sys/fs/bpf` | `<runtime>/bpf` | `bpf` | per boot |
| init's `mount none SRC DST bind` between `rw` areas | SRC's host path | `rw` | added when init runs it; with `rec`, the entries below SRC are copied below DST |
| everything else | `<image>/...` | `root`, read-only | derived image |

`<runtime>` defaults to `<data>/run` and is recreated on every boot. An
init bind mount rewrites the file, so processes started afterwards see it
(the data mirrors zygote binds app data from); running processes do not.
vold's bind of `/data/data` onto `/data/user/0` (`prepare_special_dirs`)
reaches every process and `/data_mirror/data_ce/null/0` through `/data`'s
shared propagation on Android, so it is an entry from the start; init's
`bind rec` of `/data/user` copies it to the mirror. A symlink there would
be followed in each process's own view, and zygote's tmpfs over
`/data/data` in an app would hide the app's CE storage (#234).

Resolution rules:

1. The **longest matching guest prefix** wins, on whole path components.
   `/apex/apex-info-list.xml` is a file inside the read-only `/apex`.
2. The passthrough host devices (`/dev/null`, `/dev/zero`, `/dev/random`,
   `/dev/urandom`, `/dev/tty`) take precedence over the `/dev` entry, as
   they do today in `vfs.rs`.
3. Paths resolve **component by component**, and symlinks are interpreted
   relative to the guest root wherever they are stored. Examples are `/bin`
   in the image and `/dev/stdin -> /proc/self/fd/0` in the runtime `/dev`.
   Each intermediate result goes through the map again.
   `PathMap::resolve` in `src/paths.rs` is the reference implementation.
   A process's own mount (`mount(2)` in the layer) hides the older entries
   at and below its mount point, as mounting over a directory does.
4. `kernfs` entries are not a view of `/proc` or `/sys`. The layer keeps
   synthesizing those (`/proc/self/*` first). See section 7 for what the
   tree contains.
5. The inverse map (host path → guest path) must use the same table. This
   covers `F_GETPATH`, `/proc/self/fd/N`, `getcwd` and `dl_iterate_phdr`
   names. A host path under `<runtime>/dev` is guest `/dev/...`.
6. `rw` areas are real host directories. `mkdir`, `unlink`, `rename`,
   `O_CREAT` and so on act on the host. The `root` image is read-only to
   the guest: writes fail with `EROFS`.

## 3. `--inherit-env`

The guest `envp` is `linux-run`'s own environment, verbatim, in order.
guest-init builds it exactly as init does:

1. `PATH=` `_PATH_DEFPATH`;
2. `export` / `load_exports` values, in order (`init.environ.rc`,
   `/data/system/environ/classpath`, ...);
3. the service's `setenv` lines;
4. `ANDROID_SOCKET_<name>=<fd>` and `ANDROID_FILE_<path>=<fd>`
   (non-alphanumerics become `_`).

Without the option, `linux-run` keeps using `default_android_env()`, which
drops the descriptor variables.

## 4. `--identity FILE`: credentials

Darwin runs every service as the host user. The Linux credentials init would
set before `execv` are in a per-process file:

```
# aim-guest-init identity v1
service	logd
uid	1036
gid	1036
groups	1000 1032 3009
cap_effective	0x440000000
cap_permitted	0x440000000
cap_inheritable	0x440000000
cap_ambient	0x440000000
cap_bounding	0x440000000
seclabel	
priority	10
oom_score_adj	-1000
rlimit	7	32768	32768
rlimit	13	40	40
```

- Integers are decimal, capability masks are hex (bit n = Linux capability
  n). Limits are decimal or `unlimited`. The `rlimit` resource is the Linux
  `RLIMIT_*` number. Later lines for the same resource override earlier ones:
  init's `setrlimit` lines come first, then the service's `rlimit` options.
  Unknown keys must be ignored.
- The capability sets follow init. An explicit `capabilities` line fills
  every set and bounds the bounding set to it. root without that line has
  every capability permitted and effective. Any other uid has none.
- The layer reports from this state:
  - `getuid`, `geteuid`, `getresuid`, `getgid`, `getegid`, `getresgid` and
    `getgroups`;
  - `capget`, `prctl(PR_CAPBSET_READ)` and `prctl(PR_CAP_AMBIENT,
    PR_CAP_AMBIENT_IS_SET)`;
  - `getrlimit` and `prlimit64` for the guest's own pid;
  - `getpriority` (`priority`);
  - `/proc/self/status` (`Uid:`, `Gid:`, `Groups:`, `CapInh/CapPrm/CapEff/
    CapBnd/CapAmb:`);
  - `/proc/self/oom_score_adj`;
  - `/proc/self/attr/current` (`seclabel`, or `u:r:init:s0` when empty).
- `setuid`, `setgid`, `setresuid`, `setgroups`, `capset`,
  `prctl(PR_CAPBSET_DROP)` and `PR_SET_KEEPCAPS` update the in-process
  state with Linux permission rules. They do not change the file.
- **Children.** A forked child inherits the in-process state
  (`docs/fork.md`). The layer writes the child's identity to
  `<dir of FILE>/by-pid/<host pid>`, where guest-init already links each
  service's pid. It removes the file when the process exits. The by-pid
  directory is the process table for peer credentials:
  - `SO_PEERCRED` and `SCM_CREDENTIALS` on AF_UNIX sockets must report the
    peer's guest pid, uid and gid from there (`LOCAL_PEERPID` →
    `by-pid/<pid>`).
  - guest-init itself does the same for `property_service` peers.
  - A pid with no entry is reported as root.

## 5. Inherited descriptors

- `socket` lines are created by guest-init before the launch:
  `socket(AF_UNIX, type)`, then `bind` at the mapped `/dev/socket/<name>`,
  `chmod` to the listed mode, and `listen` only for `+listen`, as init's
  `CreateSocket` does.
- `file` lines are opened with their mode.
- Each descriptor arrives at a fixed number: sockets first, from fd 3, then
  files, in declaration order. The number is in `ANDROID_SOCKET_*` /
  `ANDROID_FILE_*`. A `file` that cannot be opened is not published, as in
  init.
- The layer must not close, reuse or `CLOEXEC` fds 3 and up before the guest
  starts. Its own descriptors must be allocated above them
  (`F_DUPFD_CLOEXEC` from a high base).
- `<runtime>/sockets` records each created socket, one tab-separated line
  per creation, last line wins:

  ```
  <service>	/dev/socket/<name>	<guest type>	<host type>	passcred|-	listen|-
  ```

  - Darwin has no AF_UNIX `SOCK_SEQPACKET`. Such sockets are created as
    `SOCK_STREAM`, and the layer must give the guest seqpacket semantics
    (record boundaries, `getsockopt(SO_TYPE)` = `SOCK_SEQPACKET`).
    `/dev/socket/logdr` is one of these.
  - `passcred` (`dgram+passcred`, for example `logdw`) means the layer must
    attach `SCM_CREDENTIALS` with the sender's guest identity (section 4)
    to every received message. Darwin has no `SO_PASSCRED`.

### Unix socket paths

Guest `bind`/`connect` on an AF_UNIX path must map the path (section 2). The
host path can exceed Darwin's 104-byte `sun_path`, as in any temp or data
directory. guest-init therefore binds and connects **by the short name,
relative to the socket's directory**: it makes that directory the calling
thread's working directory with `pthread_fchdir_np(dirfd)` for the duration
of the call (`src/unixsock.rs`). The layer should do the same. It must return
the guest path, not the host path, from `getsockname`/`getpeername`.

## 6. Ownership and modes: on the inode, and `<runtime>/fs-attrs`

The host cannot `chown` to Android ids. The guest owner and mode of a file
with a writable host inode (the path map's `rw`, `cgroup2` and `bpf` areas,
a process's own tmpfs, and host files under kernfs) live on that inode, in
the host attribute `dev.aim.guest-inode`. guest-init writes it for init's
`chown`, `chmod`, `mkdir` modes and owners and for socket owners; the layer
for the guest's `chown`, `chmod`, `mkdir` modes and for the files a guest
creates (owned by its fs uid and gid, as on Linux). 20 bytes:

| Offset | Bytes | |
| --- | --- | --- |
| 0 | 5 | `DAGI`, version 1 |
| 5 | 1 | fields present: uid 1, gid 2, mode 4 |
| 6 | 2 | zero |
| 8 | 4 × 3 | uid, gid, mode (permission bits), little-endian |

A change merges its fields into those the inode has. Setting it needs
write access to the file, which a read-only host mode (a guest's
`open(O_CREAT, 0444)`, a socket of mode 0222) denies the owner: the writer
adds owner write for the change and restores the mode. The attribute moves
with a rename, is shared by hard links and lives as long as the file: an
owner installd gave `/data/data/<pkg>` survives the next boot (#261). A bind
shows its source's owner, and a stub in a process's own tmpfs is another
inode.

Paths with no writable host inode (the read-only image, whose directories
init `mkdir`s and `chmod`s, and `/proc` or `/sys` entries with no host file)
are recorded in the per-boot table `<runtime>/fs-attrs`, one tab-separated
line per change:

```
<guest path>	<uid|->	<gid|->	<octal mode|->
```

For each path the most recent line wins, field by field (`-` = unchanged).
The layer keys a path by its path through the entry with the shortest host
prefix (the area, not a bind into it); a file in a process's own tmpfs by
its host path. The table is small and written almost only by init at boot.

`stat`, `fstat` and `newfstatat` report the inode's attribute (its absent
fields: uid 0, gid 0, the host permission bits); without one, the table's
fields, then an image file's original owner and mode
(`dev.aim.android-inode`), then uid 0 and gid 0 with the host permission
bits. Reading an attribute costs about 15 µs on the development Mac, so
each process keeps what it read by (dev, ino) while the inode's ctime,
which every attribute change updates, stays the same. Two known
requirements:

- bionic refuses property areas that are not `root:root` or that are group-
  or world-writable. `/dev/__properties__/*` are mode 0444, and with the
  default owner they pass.
- `access`/`faccessat` must evaluate the guest identity against these
  owners and modes.

## 7. `/proc` and `/sys` values: `<runtime>/kernfs`

init's `write`, `copy`, `copy_per_line`, `hostname` and `domainname` to
`/proc/...` or `/sys/...` store the value at `<runtime>/kernfs/<same
path>`. Nothing on the host changes. The layer's `/proc` and `/sys`
emulation must return the recorded value for a path it serves. It may accept
later guest writes the same way. Values it does not emulate stay unused.

A dry run of the pinned image records 43 values (dry run, 2026-09-27):
`/proc/sys/kernel/*` (17, for example `panic_on_oops`, `kptr_restrict`,
`perf_event_paranoid`, `hostname`), `/proc/sys/net/*` (11), `/proc/sys/vm/*`
(5), `/proc/sys/abi/swp`, `/proc/cpu/alignment`, `/sys/module/dm_*`,
`/sys/block/zram0/*`, `/sys/kernel/debug/*` and `/sys/class/*`.

Kernel filesystems init mounts stay unmounted: the cgroup v1 controllers
from `cgroups.json` plus `/dev/memcg` and `/dev/stune`, configfs, tracefs,
pstore, bpf, functionfs and binderfs. Commands on their files are logged as
no-ops.

The cgroup v2 hierarchy (`Cgroups2` in `cgroups.json`) is the one
exception: zygote aborts a fork when `createProcessGroup` cannot make
`<root>/{system,apps}/uid_N/pid_M` and write `cgroup.procs`. It is the
`cgroup2` path-map entry, a plain per-boot directory: groups can be made,
joined and removed, but no controller acts on them (no freezing, no memory
limits). `SetupCgroups` creates `apps` and `system` in it as libprocessgroup's
`CgroupSetup` does, and `/proc/mounts` lists it as `cgroup2`. `/dev/kmsg` is a regular file in the runtime `/dev` until the layer
emulates the device.

## 8. Property areas and the futex

- guest-init creates `/dev/__properties__` in `<runtime>/dev/__properties__`
  in bionic's format:
  - `property_info`;
  - one file per context, and `properties_serial`, each `PA_SIZE`
    (128 KiB) and mode 0444.

  It keeps its own writable `MAP_SHARED` mapping of each file.
- Guest readers `open` the files read-only and `mmap` them `MAP_SHARED`.
  The layer must map them as host `MAP_SHARED` mappings of the same file, so
  both sides share pages.
- `__system_property_wait` and `__system_property_wait_any` sleep with
  **non-private** `FUTEX_WAIT_BITSET` (`FUTEX_BITSET_MATCH_ANY`) on a
  serial word, and have a timeout when one is given.
  - The layer must wait with `__ulock_wait2(UL_COMPARE_AND_WAIT_SHARED, ...)`.
  - The flavour must follow the **mapping** (`MAP_SHARED` file or shm), not
    `FUTEX_PRIVATE_FLAG`. `experiments/p0/03-futex` (removed; see git
    history): SHARED and non-shared waiters are separate namespaces.
  - `sys/futex.rs` picks the flavour from the flag today. That works for
    bionic's property waits, which use the shared op, but breaks for private
    ops on shared memory.
- guest-init wakes with `__ulock_wake(UL_COMPARE_AND_WAIT_SHARED |
  ULF_WAKE_ALL)` on its own mapping (`SharedAreaWaker`,
  `futex::UlockShared`). This happens after every add (global serial at
  offset 4 of `properties_serial`) and every update (the `prop_info` serial
  word, then the global serial).
  - The key is the backing object and offset, so the addresses may differ.
  - `tests/property_service.rs` checks a wake across two mappings of one
    file.
- Setting properties: guests connect to `/dev/socket/property_service`
  (0666) or `property_service_for_system` (0660) through the path map. They
  speak `PROP_MSG_SETPROP2` and get init's reply codes. `ctl.*` messages
  are answered after guest-init has acted on them.

## 9. What guest-init plays itself

- **ueventd.** `ro.cold_boot_done=true` is set before the boot, and no
  device nodes are created. `start ueventd` is a no-op.
- **apexd.** APEXes are pre-flattened. `/apex/apex-info-list.xml` is
  generated from `/apex/*/apex_manifest.pb`. Each package's partition comes
  from the partition `apex` directories, and it drives vendor-APEX
  subcontexts and linkerconfig.
  - The first `start apexd` sets `apexd.status=activated` and
    `apex.all.ready=true`. Later starts (a lazy `aidl/apexservice`
    request) change nothing: `apexservice` is not served.
  - `apexd-snapshotde` sets `ready`.
  - `apexd-bootstrap` is a no-op. `perform_apex_config --bootstrap` loads
    the scripts of the APEXes whose manifest sets `vendorBootstrap`, which
    apexd would activate then; the others' scripts load at
    `perform_apex_config`.
- **Image paths.** guest-init reads the image (scripts, `build.prop`,
  property contexts) with its symlinks resolved relative to the guest root,
  as `linux-run` does.
- **linkerconfig.** `perform_apex_config` and `update_linker_config` run the
  original `/apex/com.android.runtime/bin/linkerconfig --target
  /linkerconfig` as root through `linux-run`, and wait for it.

## 10. First launches (2026-09-27)

`guest-init --run --only servicemanager,logd` against the pinned image. That
`linux-run` has none of the options above.

- `linkerconfig`, `servicemanager` and `logd` all stop in `linker64` before
  `main`.
- Unimplemented:
  - `mremap` (216) from the CFI shadow (`linker_cfi.cpp:77 ~ShadowWrite
    CHECK 'res != MAP_FAILED'`);
  - `socket(AF_UNIX, SOCK_SEQPACKET|SOCK_CLOEXEC)` (198), liblog's
    `logdw`;
  - `rt_tgsigqueueinfo` (240), abort's signal.
- The processes exit with status 127.
- servicemanager is `critical`. After five crashes guest-init reports what
  init would do: `critical process 'servicemanager' exited 4 times before
  boot completed`.
- Tracked in #174.

The current state of the boot is in [boot-status.md](boot-status.md).
