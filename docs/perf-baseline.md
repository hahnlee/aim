# Performance baseline

`cargo aim bench` is the yardstick for performance work on the stack
(fork cost, binder, graphics fences, boot trimming): run it before and
after a change and compare the two reports.

```sh
cargo aim bench --runs 3                  # three boots; table of medians on stdout
cargo aim bench --compare A.json B.json   # the change of every median, A to B
```

Each run writes nothing outside `target/aim/bench/`: the report
`<timestamp>.json` (UTC), the guest-init log of each run, the fixtures in
`bin/` and the data directories `data-<n>` (removed after the run unless
`--keep`). The report holds the git sha (and whether the tree was dirty),
the machine (model, CPU, cores, memory, macOS), the method, every run's
metrics and each metric's median, min, max and count.

## Method

Every run is one boot of a **fresh data directory** with the flags of
`cargo aim boot`: aim-display (`--size 1080x1920`) and `guest-init --run
--exclude bootanim --gpu <ANGLE> --display <socket>`. `--exclude bootanim`
is a measurement flag, as in the tests (docs/boot-status.md, "Host security
agent"); the bench changes no image, device configuration or setting.

Guest commands (`getprop`, `logcat`, `am`, `pm`, `input`, `dumpsys`, the
fixtures) run as root through another `linux-run` on the boot's path map and
binder host. Each launch or page load is repeated three times per run; a
run records their median. Metric names end in their unit; lower is better
except for `_count`.

| Metric | Signal |
| --- | --- |
| `boot.completed_s` | `getprop sys.boot_completed` polled every 0.5 s, from guest-init's spawn (±0.5 s) |
| `boot.zygote_s`, `boot.system_server_s`, `boot.pms_ready_s`, `boot.ams_ready_s`, `boot.enable_screen_s` | wall-clock times of `boot_progress_start`, `_system_run`, `_pms_ready`, `_ams_ready`, `_enable_screen` in the events log (`logcat -b events -v epoch`), from guest-init's spawn |
| `boot.processes_count`, `boot.rss_mb` | host `linux-run` processes whose command line names the boot's path map (services, fork children, exec'd programs) and the sum of their RSS, at boot completion. Shared pages count in every process. |
| `boot.settled_s` | when the guest's CPU use (the CPU time its processes gained over 5 s windows) first drops below 1.5 host cores, at most 180 s after boot completion; everything below is measured after it |
| `idle.cpu_cores`, `idle.processes_count`, `idle.rss_mb` | at that point: the guest's CPU use over 10 s, its processes and their RSS |
| `process.fork_p50_us` (p90, p99) | `fork_bench fork 200` (`crates/aim-build/src/bench/fork_bench.c`): `fork()`, the child `_exit(0)`s, `waitpid()`, timed in the guest |
| `process.fork_exec_p50_us` (p90, p99) | `fork_bench spawn 50`: the same with the child exec'ing `/system/bin/true` |
| `binder.ping_p50_us` (p90, p99) | the servicemanager ping fixture (`crates/aim-linux-abi/tests/fixtures/binder_ping.c`), `ping 5000` |
| `settings.first_start_ms`, `settings.cold_start_ms`, `settings.warm_start_ms` | `am start -W` `TotalTime` (to the first frame) of `.homepage.SettingsHomepageActivity` (`.Settings` is a trampoline, whose cold launch reports no time): the first launch (`-S`) on its own, then the median of three cold launches (`-S`, force-stop) and of three warm ones (after `KEYCODE_HOME`). A launch reported in another state than asked for (a warm launch must be WARM or HOT), without a time, or not answered in 90 s is dropped. |
| `calculator.install_s`, `calculator.*_start_ms` | the same for Calculator (`first` is its first launch after install) from `_build/installed-apps` (installed from a copy in `/data/local/tmp`) |
| `chrome.install_s` | `pm install -r -g` of a copy of `_build/installed-apps/org.chromium.chrome`, host-timed |
| `chrome.first_start_ms`, `chrome.cold_start_ms`, `chrome.warm_start_ms` | as above. The first-run screens are skipped with Chrome's own command-line file (`--disable-fre --no-first-run --no-default-browser-check`), read for the debug app (`am set-debug-app --persistent`) |
| `chrome.load_example_s`, `chrome.load_wikipedia_s` | from `am start -a VIEW -d URL` (a distinct `?aimbench=i` each time, so each is a new navigation) to the last change of the screen before it stays unchanged for 3 s ("visually complete"). The screen is aim-display's last presented buffer (its SIGUSR1 capture), hashed every ~0.2 s. The first load of a page has a cold HTTP cache, the others a warm one. A load does not count when a Chrome child process (renderer, GPU) died during it (`Child process died` in logcat) or when the middle 60 % of the settled screen is one color (nothing drawn). |
| `<app>.scroll_janky_pct`, `<app>.scroll_frame_p50_ms` (p90, p99), `<app>.scroll_frames_count` | `dumpsys gfxinfo PKG reset`, ten `input swipe`s (five down, five up, 300 ms each), then `dumpsys gfxinfo PKG` (its first process; nothing when it rendered no frame). For Chrome these are the browser's HWUI frames (toolbar, bottom bar); the page itself is composited by Chrome's GPU process and is not in them. `dumpsys SurfaceFlinger --latency` would cover it, but returns no frame history on this image. |
| `chrome.processes_count`, `chrome.rss_mb` | as `boot.*`, with Chrome running and the Wikipedia page loaded |

Why these signals: `am start -W` is the platform's own launch metric;
Chrome has no stable "page loaded" log line, and its web content never
passes through HWUI, so the only signal that covers the whole page (layout,
images, the progress bar ending) without instrumenting Chrome is the
composited screen. Its resolution is the ~0.2 s poll; a page with endless
animation would never settle and is cut at 60 s.

The guest is far from idle after a first boot: GMS, the launcher and
dexopt keep it at 3–11 host cores for minutes (`idle.cpu_cores`), and it
never dropped below 1.5 cores within 180 s in these runs, so `boot.settled_s`
is the 180 s cap. Everything after boot is therefore measured on a busy
guest; compare A/B runs made the same way in the same session. "Idle CPU
after boot" below has what runs.

## Baseline (preliminary)

Main at `52c3bf9a` (the bench on `agent/bench`), M2 Pro (Mac14,10, 12 cores,
16 GB), macOS 27.0, 2026-09-28/29. Four runs: three in one `cargo aim bench
--runs 3` and one more (`--runs 1`) on an idle machine with the host network
checked first. Median, range and each run's value (runs 1–3, then 4):

| Metric | Median | Range | Runs |
| --- | --- | --- | --- |
| `boot.zygote_s` | 4.63 | 4.57–4.66 | 4.66 / 4.63 / 4.64 / 4.57 |
| `boot.system_server_s` | 5.89 | 5.86–5.93 | 5.93 / 5.92 / 5.87 / 5.86 |
| `boot.pms_ready_s` | 12.4 | 12.2–12.5 | 12.5 / 12.5 / 12.2 / 12.2 |
| `boot.ams_ready_s` | 29.5 | 28.9–30.3 | 30.3 / 28.9 / 29.6 / 29.5 |
| `boot.enable_screen_s` | 40.9 | 38.1–42.1 | 42.1 / 40.2 / 38.1 / 41.6 |
| `boot.completed_s` | 49.5 | 46.2–52.3 | 52.3 / 49.3 / 46.2 / 49.7 |
| `boot.processes_count` | 72 | 70–72 | 72 / 72 / 72 / 70 |
| `boot.rss_mb` | 4,390 | 4,027–4,471 | 4,411 / 4,471 / 4,027 / 4,370 |
| `idle.cpu_cores` | 6.9 | 3.4–10.7 | 10.7 / 6.65 / 3.37 / 7.15 |
| `idle.processes_count` | 126 | 124–126 | 126 / 126 / 124 / 126 |
| `idle.rss_mb` | 8,222 | 5,977–8,315 | 8,315 / 5,977 / 8,204 / 8,241 |
| `process.fork_p50_us` (p90, p99) | 10,871 (20,076, 51,346) | p50 10,281–11,419 | 11,359 / 10,383 / 10,281 / 11,419 |
| `process.fork_exec_p50_us` (p90, p99) | 59,062 (111,752, 193,082) | p50 52,906–80,347 | 80,347 / 58,178 / 52,906 / 59,945 |
| `binder.ping_p50_us` (p90, p99) | 104 (626, 1,926) | p50 96–128 | 101 / 108 / 128 / 96 |
| `settings.first_start_ms` | 2,994 | 1,539–3,355 | 3,317 / 2,672 / 1,539 / 3,355 |
| `settings.cold_start_ms` | 3,427 | 1,865–5,468 | 5,468 / 3,414 / 1,865 / 3,440 |
| `settings.warm_start_ms` | 656 | 515–738 | – / 656 / 515 / 738 |
| `settings.scroll_janky_pct` | 73.1 | 32.0–85.1 | 85.1 / 75.6 / 32.0 / 70.7 |
| `settings.scroll_frame_p50_ms` (p90, p99) | 115 (200, 250) | p50 24–150 | 150 / 113 / 24 / 117 |
| `calculator.install_s` | 0.99 | 0.56–3.26 | 3.26 / 1.17 / 0.81 / 0.56 |
| `calculator.first_start_ms` | 3,282 | 2,958–3,519 | 2,958 / 3,519 / 3,071 / 3,493 |
| `calculator.cold_start_ms` | 4,423 | 3,343–5,697 | 3,756 / 5,697 / 3,343 / 5,090 |
| `calculator.warm_start_ms` | 652 | 602–756 | 620 / 602 / 756 / 685 |
| `chrome.install_s` | 11.9 | 5.2–13.0 (109) | 13.0 / 109 / 10.8 / 5.22 |
| `chrome.first_start_ms` | 6,319 | 4,130–8,446 | 6,791 / 5,847 / 4,130 / 8,446 |
| `chrome.cold_start_ms` | 6,338 | 3,080–7,626 | 7,569 / 3,080 / 5,107 / 7,626 |
| `chrome.warm_start_ms` | 1,843 | – | – / – / 1,843 / – |
| `chrome.load_example_s` | 5.6 | – | (discarded) / 5.58 |
| `chrome.load_wikipedia_s` | 6.4 | – | (discarded) / 6.40 |
| `chrome.scroll_janky_pct` | 72.2 | 28.8–94.1 | 94.1 / 28.8 / 63.6 / 80.8 |
| `chrome.scroll_frames_count` | 24 | 17–153 | 17 / 153 / 22 / 26 |
| `chrome.processes_count` | 140 | 130–142 | 142 / 130 / 139 / 142 |
| `chrome.rss_mb` | 7,389 | 6,564–7,643 | 6,564 / 7,460 / 7,643 / 7,319 |

The JSON reports are `20260928-152636.json` (runs 1–3) and
`20260928-155253.json` (run 4); the fork/binder percentiles in parentheses
are the medians of each run's p90 and p99.

**Why preliminary, and what was discarded:**

- During runs 1–3 the machine was in use and the Mac's LAN cable was
  unplugged for part of the time. Their page loads are discarded (example.com
  7.7 / 8.7 / 1.6 s, Wikipedia 2.5 / 2.0 / 2.8 s); run 2's Chrome install
  (109 s) is an outlier and is left out of the range. Run 4, on an idle
  machine with the network up, agrees with runs 1–3 on everything else, so
  their boot, fork, binder, start and memory numbers are kept.
- Run 1's `settings.warm_start_ms` was a launch reported WARM with
  `TotalTime: 0`; the bench now drops such launches. Most warm launches after
  HOME report `LaunchState: UNKNOWN` without a time (#279), so the warm
  medians rest on one or two launches per run, and Chrome's on one in all.
- Run 4's page loads rest on one or two loads each: one example.com load
  settled on a blank page area and one Wikipedia load saw a Chrome child
  process die, and the bench dropped both.
- `idle.cpu_cores` shows why app starts spread so much: after a first boot
  the guest itself stays at 3–11 host cores (GMS, launcher, dexopt, and the
  Widevine HAL's fault loop, #274). The same Chrome cold start takes
  1.35–1.49 s on a guest that has calmed down (see below) and 3.1–7.6 s in
  the bench.
- Two trial runs made before the translation cache was indexed for this
  checkout (every ELF rewritten at load time, #275) booted in 19.0 and 26.6 s
  (`boot.completed_s`) against 46–52 s here; the time between
  `pms_ready` and `ams_ready` grew from 5.7 to 17 s. Whether the cache path
  or the post-boot load explains that needs an A/B run.

## Idle CPU after boot (#328)

`cargo aim boot`s of fresh data directories, 2026-09-29, on the shared
M2 Pro (host load average 3–150 from other agents' builds and boots, so
the numbers locate, they do not compare). The host CPU of each guest
process over 10–20 s windows (`ps` time deltas), named by the guest:
services by their identity file, app processes by `dumpsys activity
processes` (another process's `/proc/<pid>/cmdline` is empty for fork
children, #238).

At `a807c311`, two minutes after boot completion, the guest used 9.6–10.8
cores: Play Store (installing and dexopting) 3.1, system_server 0.9, the
launcher 0.9, servicemanager 0.7, GMS and GMS persistent 0.7, then 60
processes under 0.4 each. Five minutes after boot it was still 7–8 cores,
with system_server 1.6, netd 1.0 and NetworkStack 0.9 on top. What of
that was the layer's or the device's, and what became of it:

| Loop | Cost | Now |
| --- | --- | --- |
| eth0 went down and up about three times a second: DhcpClient's `connect` to the server reached the host, where the Mac's own DHCP client holds that address pair (EADDRINUSE), so every lease failed | system_server, netd, NetworkStack and every app's network callbacks (24 CONNECTED broadcasts in 9 s) | the connect goes to the virtual router; one lease per boot |
| UwbService and FingerprintService waited for their removed HALs in system_server's binder threads, three times a second | binder threads, ctl.interface_start requests | the device no longer declares UWB or a fingerprint sensor |
| traced_probes aborted about once a minute on its memory watchdog | a restart and a cleanup exec each time | `/proc` counts rss in 16 KiB pages; its watchdog had read four times its resident size |
| GMS persistent dies every 2 s ("UsbManagerCompat is unavailable": no `usb` service) | a new process start each time, plus GMS service restarts, runtime-permission rewrites and package events | the device declares `android.hardware.usb.host`, so UsbService runs (#336); sysfs no longer shows init's `/sys/class/android_usb`, and uevent sockets work |

Six minutes after boot completion, over 2 minutes sampled every second
(processes that came and went included), `55cb4474` used 0.05 cores (load
2–13) against 1.73 on main (load 12–25): main's loop still started GMS
persistent 6 times in that window, and each round restarted GMS, the Play
Store and other apps (0.83 cores in processes that started in the window).
Two minutes after boot completion both still run first-boot work
(dex2oat, app starts): 1.2 and 5.0 cores against 3.2 on main, of which
GMS persistent's own processes took 0.76 on main.

Most of the services' CPU at boot is system time, with millions of page
faults: servicemanager 2:06 of CPU of which 1.96 s user and 9.3 million
faults, SurfaceFlinger 2:47 with 3.3 s user, system_server "0.3% user +
6.1% kernel" in its own ANR report. It happens while the host is short of
memory (vm_stat: 60–300 MB free, 100,000–700,000 page reactivations per
20 s) and stops when memory is free again; a binder ping costs
servicemanager no faults. It is the host's paging, not a loop.

## Where the time goes: exec, process start and the early boot

Measured 2026-09-29 on the M2 Pro with timestamps (not committed) at
guest-init's launches, `linux-run`'s `main`, the guest's first
instruction, `execve` and `exit_group`, and syscall traces with times
(`--trace`). Boots of fresh data images with `cargo aim boot`: "full"
(load average 5 at the start), and "quiet" with `--exclude zygote` (load
5), where the microbenchmarks ran: `fork_bench`, an exec benchmark (a
fork child stamps the time into shared memory and execs
`/system/bin/true`; to the parent seeing it exit) and
`__system_property_set` in a loop. A is main at `dbde2b0a`, B the
changes below; each boot was the first after rebuilding (a cold host
cache: the security agent's per-file decisions, the image's page cache),
B3 a second boot of B.

| | A1 | A2 | B1 | B2 | B3 (warm) |
| --- | --- | --- | --- | --- | --- |
| first service launched (guest-init's preparation) | 1.82 | 1.61 | 1.80 | 1.53 | 1.41 |
| zygote launched | 5.54 | 5.33 | 3.89 | 3.49 | 2.88 |
| `boot_progress_start` | 6.50 | 6.28 | – | 4.31 | 3.54 |
| `boot_progress_system_run` | 9.50 | 9.20 | – | 7.51 | 6.22 |
| `boot_progress_pms_ready` | 15.9 | 15.3 | – | 13.7 | 11.9 |
| `boot_progress_enable_screen` | 37.8 | 39.1 | – | 36.8 | 34.8 |
| exec of `true` to its exit, p50 (ms) | 28.4 | 28.0 | 11.5 | 11.0 | 10.4 |
| fork+exec+exit+wait, p50 (ms) | 33.6 | 34.0 | 15.7 | 15.3 | 15.3 |
| fork+exit+wait, p50 (ms) | 9.1 | 9.3 | 9.5 | 9.2 | 9.4 |
| `__system_property_set`, p50 (ms) | 7.54 | 7.52 | 0.063 | 0.061 | 0.044 |

Boot times are seconds since guest-init's start (B1's events log was not
read before its boot ended). A quiet guest's fork+exec of `true`, stage
by stage (p50, ms), before and after:

| | A | B |
| --- | --- | --- |
| fork in the parent | 0.7 | 0.6 |
| the child `linux-run`'s spawn, dyld, to `main` | 3.8 | 3.1 |
| receiving and mapping the parent's memory | 0.9 | 0.7 |
| execve: Darwin's execve of `linux-run`, dyld, the layer's start | 4.3 + 1.3 | – |
| execve in place: close-on-exec, unmap, reset, load | – | 1.1 |
| `true` from its first instruction to `exit_group` | 21.8 | 9.0 |

What changed, largest first:

- **Property-area mappings** (#337). A host file mapping of a property
  area cost 0.8-3 ms per call in a `linux-run` process, every time, while
  guest-init held the file mapped writable: the host's security agent
  (a fresh copy of the same file maps in 14 µs once checked). bionic maps
  12-14 areas in every program it starts, half of `true`'s run. guest-init
  now shares a memory entry of each area through the binder host, and the
  layer maps that (3-7 µs), `sys/sharedfile.rs`.
- **guest-init's child exits**: the boot loop saw an exit only at its next
  100 ms timeout, 35-175 ms of idle time after every one-shot `exec`
  service (about 1 s before zygote); it now wakes on SIGCHLD.
- **setprop**: the property service's accept threads slept 5 ms between
  polls (7.5 ms per `setprop` with the timer slack); they now wait in
  `poll`.
- **execve in place** (`sys/exec.rs`): the Darwin re-exec of `linux-run`
  (dyld, the layer's start, and every per-process cache lost) is replaced
  by an in-place image replacement where the process allows it; a boot
  execs 62 programs in place and 3 anew (multi-threaded parents).
- **Translated ELFs with one host open** (#339): no read of the original
  (a decompression in the compressed image, 2-4 ms per library on a cold
  cache) and one host open instead of two (170 -> 84 µs per library).

What remains is mostly the host security agent, whose per-call costs a
`linux-run` process pays on the repository's paths (the data image and
the derived image included), measured in a quiet guest:

- **every host `open`: 70-90 µs** (`stat` 8 µs), also `O_PATH` and
  `O_EVTONLY`. `true` still makes 60 opens (the linker's search paths,
  `ld.config.txt`, the property areas, 12 libraries): about 5 ms of its
  9 ms. A static program or a phone does this in about 1-2 ms;
- **the first mapping of a file in a process: about 0.9 ms** (p50), the
  next ones 11 µs. zygote's start (traced) maps 278 files, 274 ms; it
  takes 0.85 s from its first instruction to `boot_progress_start`.
  `true`'s 12 libraries, mapped by every process, stay cheap;
- **writes: an `O_CREAT|O_TRUNC` open about 1 ms, `unlink` 1 ms, `rename`
  3.6 ms, `BPF_OBJ_GET` (an `O_RDWR` open) 2.3-4.8 ms**:
  `mainline_aconfigd_init` (165 ms) and `bpfloader` (316 ms, with 95,000
  4-KiB `read`s of its programs) are the longest one-shot services
  before zygote.

The early boot on B (B3): guest-init prepares for 1.4 s, then about 25
services run in series until zygote is launched at 2.9 s: 8-15 ms from launch to a service's first instruction (the spawn
and dyld about 4 ms, the program's first mapping about 1 ms, the by-pid
entry's write about 2 ms), 0.3 s of `bpfloader`, 0.35 s of the aconfigd
services, and 0.15 s where guest-init itself runs the atrace `chmod`s
and other builtins of `late-init`. zygote then needs 0.85 s to
`boot_progress_start`.

## Where the time goes: Settings scroll

`dumpsys gfxinfo com.android.settings framestats` over ten `input swipe`s
(120 frames), SurfaceFlinger's `--frametimeline` and `--timestats`:

- The app did about 1 ms of work per frame (input, animation, traversal,
  draw, issue). The rest was waiting: the RenderThread about 30 ms p50 in
  `dequeueBuffer`, the UI thread about 30 ms for the RenderThread (sync),
  so frames completed 90 ms after their vsync and 92–96 % were janky.
- Every SurfaceFlinger frame was "deadline missed (while in GPU comp)":
  it started 17 ms late and took 33 ms, and presents came 33 ms apart.
  A frame is on screen, and its present fence signals, about two vsyncs
  after SurfaceFlinger presents it (the window's compositor). With two
  client targets RenderEngine waited for the one it drew two frames ago,
  and GL backpressure skipped a vsync while a present fence was pending.
  With three client targets and no GL backpressure (docs/composer.md,
  "Frames in flight"): presents 16.7 ms apart, 4–7 % janky frames and
  p50 18–19 ms at host load ~25, 30–63 % at load 45.
- gfxinfo's "gpu percentile: 4950ms" (#324) is its GPU histogram's last
  bucket, which holds every frame whose GPU completion came more than
  25 ms after its swap started; framestats' `GpuCompleted` values are in
  the monotonic clock of the rest, 1–65 ms after `SwapBuffers`, the long
  ones those whose `dequeueBuffer` waited. There is no clock error.

## Where the time goes: Chrome cold start

Measured on a live guest (`cargo aim boot`) after its post-boot churn had
mostly calmed down: three `am start -W -S` of Chrome, TotalTime 1,486,
1,444 and 1,348 ms. For each launch a host script found the new fork child
of zygote and ran `sample <pid> 2 1` on it from its first ~50 ms, and
`sample <zygote> 4 1` over the launch; phases come from the events log.

**Phases** (median of three; ms):

| From → to (events) | ms |
| --- | --- |
| launch request → zygote fork returned (`am_proc_start`) | 117 |
| fork → `bindApplication` (`am_proc_bound`): child startup, attach | 368 |
| `bindApplication` → activity launch (`wm_restart_activity`) | 330 |
| activity `onCreate` → `onResume` | 243 |
| `onResume` → first frame (`wm_activity_launch_time`) | 326 |
| sum (TotalTime median 1,444) | 1,384 |

**Where the host time goes, ranked** (browser main thread, the first 2 s
after the fork: about 1,040 samples at ~1.9 ms, three runs; zygote's main
thread over the launch; plus direct timings):

1. **fs-attrs lookups: ~550–600 ms** (28–31 % of the browser main thread).
   Every `newfstatat`/`fstat` goes through `sys::attrs::lookup`, which
   re-reads and re-parses the whole `run/fs-attrs` table (5 MB, 51,800 lines
   after a first boot) whenever any process changed it, under one mutex:
   18–21 % is waiting for that mutex, most of the rest is the parse. The
   same path costs zygote 50–100 ms per fork (it fstat's every fd first) and
   every exec'd program ~24 ms (`linux-run /system/bin/true`: 16 ms without
   a path map, 40 ms with one). #273.
2. **Guest code: ~550–650 ms** (27–33 %): ART, the framework and Chrome
   itself (Java and native), i.e. the real work of the start.
3. **Waiting for other threads: ~300 ms** (futex, 15–16 %), including the
   RenderThread's GPU setup (item 6).
4. **Binder round trips: ~150–180 ms** (7–9 %; `binder::ioctl` →
   `mach_msg`), mostly the attach/bind/activity calls to system_server. The
   raw round trip is 96–128 µs p50 but 0.3–1.1 ms p90 and 1–3.6 ms p99 on the
   busy guest (`binder.ping_*`).
5. **Load-time rewrite of the app's code: ~120 ms** (6 %): Chrome's own
   code has no translation-cache entry, so `mmap` scans it word by word in
   every process on every start. #278.
6. **GPU setup per process: ~110 ms** on the RenderThread
   (`aim_host_gpu::call`), of which `dlopen` of ANGLE ~75 ms and Metal device
   creation (`MTLCreateSystemDefaultDevice`) most of the rest; waits for
   present were not visible (the RenderThread otherwise sits in
   `epoll_wait`).
7. **Fork: ~30 ms** in zygote's `sys::fork` (memory entries, state, spawn of
   the child `linux-run`) of the 117 ms launch→fork phase; the rest is
   zygote's pre-fork fd check (item 1) and system_server. The guest
   microbenchmark: fork+exit+wait 10.3–11.4 ms p50, fork+exec 53–80 ms p50.
8. **`linux-run` spawn (dyld, frameworks): ~3 ms** per process
   (`linux-run --help` 5.5 ms against 2.2 ms for `/usr/bin/true`), which is
   negligible.

The `/proc/self/maps` reads (`procfs::maps`, `vmmap::region_at`) are another
2–4 % (~50 ms). Beyond Chrome, the largest single lever on the measured
numbers is the busy post-boot guest (`idle.cpu_cores` 3–11): the same cold
start is 2–5× slower in the bench than on a calmed-down guest.

## Start-up and fork measurements (#190, #239)

Microbenchmarks, not `cargo aim bench`: release builds, M2 Pro,
2026-09-29, on a host other agents also loaded; A is main at `b6967392`,
B the change, run back to back. Treat them as indicative.

- **ART start-up from the translation cache (#190):** the 2x gap
  (0.73 s cached against 0.34 s load-time) was not the cache. The host's
  endpoint security agent authorizes a process's first `mmap` of each file
  outside `~/Library` and the temporary directory, about 0.7 ms each;
  `tests/art.rs` kept its cache in the target directory, so `dalvikvm64`
  paid it for ~310 cached libraries while load-time rewriting maps no
  files. With the cache in the temporary directory (the default cache is
  in `~/Library/Caches`), the cache is as fast as load-time rewriting:
  0.39–0.46 s against 0.33–0.49 s over the test's four configurations
  (it was 0.64–0.81 s). Image files mapped from the repository's derived
  image still pay it (#295).
- **`linux-run` start** (spawn, `--help`, exit; p50 of 200): 7.2 → 4.7 ms
  once the host frameworks are opened on first use (`/usr/bin/true`:
  4.1 ms). docs/fork.md has the fork numbers: fork+exit+wait about
  1.5 ms and fork+exec about 4.5 ms faster.

An interleaved `cargo aim bench` A/B was started, but the host's load
average was 95–295 on 12 cores during it, and the runs spread 2–4x
between runs of the same build (`process.fork_p50_us` 13 ms in one run,
56 ms in the next), so none of it is reported here; a bench of main after
the merge replaces it.
