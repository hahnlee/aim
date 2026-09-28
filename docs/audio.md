# Audio HAL (ADR 0012, P5)

The original audioserver (AudioFlinger and AudioPolicyService) plays and
records through our vendor audio HAL, which plays and records through
CoreAudio. It is the new runtime's form of #5's option B; the old stack's
interim path (option A) stays until the switch in P6.

| Piece | Where |
| --- | --- |
| HAL service: `IModule/default`, `IConfig/default` (audio.core V3) and `IFactory/default` (audio.effect V3) in one process | `hal/audio`, `/vendor/bin/hw/android.hardware.audio.service-aidl.aim` |
| FMQs laid out as libfmq's (both sides) | `hal/fmq` (`aim-fmq`) |
| Host module `audio` (id 7): the shared ring, the null sink, the CoreAudio process's client | `crates/aim-host-audio`, contract in `aim_hostcall::audio` |
| CoreAudio process (`linux-run --audio-io`): device queries and AUHAL units | `crates/aim-host-audio/src/io.rs` |
| `.rc` (service `vendor.audio-hal-aidl`) and vintf fragment | `hal/audio/*.rc`, `hal/audio/*.xml`, placed by `image/overlay.toml` |
| HAL test client (not in the image) | `hal/audio/check` (`audio-hal-check`) |
| Tests | `crates/aim-host-audio` (unit, device), `crates/aim-linux-abi/tests/audio.rs` (boot) |

## How audioserver finds it

libaudiohal's `FactoryHal` picks the AIDL HAL when
`android.hardware.audio.core.IModule/default` is declared, and requires an
effect HAL of the same kind: without `audio.effect.IFactory/default` it
asks the HIDL service manager, and waits for it. Our fragment declares all
three instances; the service is named `vendor.audio-hal-aidl`, the name
audioserver's `.rc` restarts with it.

With an AIDL HAL, AudioFlinger builds the policy configuration from the
HAL (`getAudioPolicyConfig`): the modules' ports and routes, and
`IConfig`'s surround and engine configuration. The vendor's
`audio_policy_configuration.xml` files are not read; they stay in the
image unused, as the emulator's HIDL HAL left them. #187 already removed
the emulator's audio HAL (its `.rc`, its `IDevicesFactory` fragment,
`bluetooth_audio.xml` and the HIDL `IEffectsFactory` in `manifest.xml`);
nothing else of it remains to remove.

## Start-up

The service registers `IConfig`, `IModule` and `IFactory` at once, before
anything touches CoreAudio: audioserver waits for them, and system_server's
`AudioService` waits for audioserver, so a HAL stuck on CoreAudio stops the
whole boot (#217). The module asks the host for the devices on a thread of
its own right after start; the first binder call that needs the ports
waits for that answer, which comes within 3 seconds.

## The primary module

Built from the Mac's default devices (`FN_DEVICES`), in the
shape of AOSP's reference primary configuration. A device the Mac lacks is
left out, and audioserver takes its no-device path.

| Port | Kind | Profiles |
| --- | --- | --- |
| Speaker | device `OUT_SPEAKER`, default | dynamic |
| primary output | mix, flag `PRIMARY` | PCM 16 and float, stereo, 48 kHz |
| Built-In Mic | device `IN_MICROPHONE`, default | dynamic |
| primary input | mix, at most one active stream | PCM 16 and float, mono and stereo, the microphone's rate |

Routes: primary output to Speaker, Built-In Mic to primary input. Output
is always 48 kHz: the DefaultOutput unit converts to the device (and
follows the user's choice of output device). Input runs at the
microphone's own rate, since AUHAL does not convert rates on capture; a
mono microphone feeds every channel.

Port configs, patches and stream bookkeeping follow the reference
`Module.cpp` (what libaudiohal's `Hal2AidlMapper` and VTS expect). A
stream's nominal latency is one device buffer (512 frames, 11 ms, on a
MacBook Pro), so a patch's minimum stream buffer is 528 frames, and
AudioFlinger runs its FastMixer over it. There are no MMAP ports
(`getMmapPolicyInfos` is `NEVER`), so AAudio takes its legacy path.

`IConfig` returns no surround formats and an empty engine configuration:
the policy engine then uses its default product strategies. It has no
volume curves either, so every stream plays at 0 dB until volume groups
come from the image's volume XML (the reference HAL reads them from
`audio_policy_configuration.xml`). The effect factory has no effects.
Sound dose, telephony and Bluetooth are absent (null interfaces).

## A stream

```text
audioserver                  HAL process (linux-run)                    CoreAudio process's I/O thread
libaudiohal ── data FMQ ──>  worker: burst → ring write (blocks if full)
            ── command FMQ ─>        ring: memfd, lock-free SPSC   ──>  render callback
            <─ reply FMQ ───         reply: position, latency            (never waits)
```

- **FMQs.** `aim-fmq` lays queues out as libfmq's `MessageQueueBase`
  (read and write counters, ring, EventFlag word at 8-byte aligned offsets
  in one memfd), so the client's original libfmq maps them. Command and
  reply queues block with the EventFlag protocol: futex bitset waits and
  wakes on shared memory. The fixed-size `Command` and `Reply` are laid out
  as the C++ backend lays them out (an 8-bit union tag, explicit padding),
  not as the Rust backend's `Command` enum.
- **Worker.** One thread per stream runs the `StreamDescriptor` state
  machine as the reference `StreamOutWorkerLogic`/`StreamInWorkerLogic`
  do: `start`, `burst`, `drain`, `pause`, `flush`, `standby`, and the
  internal exit command that `close` sends.
- **Host ring.** Each stream has a ring in a memfd that the host module
  and the CoreAudio process map too (`aim_hostcall::audio::Ring`): single producer, single
  consumer, positions in frames, with a header the host fills in (callback
  counts and time, xruns, a peak meter, a seqlocked position stamp and a
  latency probe). The render callback copies what the ring holds and plays
  silence for the rest; it never waits for the guest. For output the ring
  holds one stream buffer plus one device buffer; a `burst` copies into it
  and sleeps while it is full, which paces the mixer. The unit starts on
  the first `burst` after `start` and stops on `pause` and `standby`.
- **Position and latency.** The callback stamps "ring frame n reaches the
  speaker at CLOCK_MONOTONIC t" (host time plus device, stream and safety
  latency). The reply's `observable` position is that stamp extrapolated to
  now and bounded by what the device has taken, so it advances at the
  device's rate and stops where the data ended. `latencyMs` is the device
  latency plus what the ring holds. `xrunFrames` counts silence played
  while the stream was active, not while idle.
- **Input.** The input unit is set up on the stream's first `start`, on a
  thread of its own: its first use asks macOS for microphone access and
  waits for the user's answer (TCC), which the HAL must not wait for.
  Until the first capture callback
  the stream delivers silence at the capture rate, as from a muted
  microphone, so AudioFlinger's record thread keeps its pace.
- **Stats.** On `standby` and `close` the worker logs what the callback
  saw: frames, callbacks and their host time, xruns, peak (dBFS) and the
  write-to-callback delay.

## The CoreAudio process and teardown

Every CoreAudio call of the HAL runs in a separate host process,
`linux-run --audio-io`, which the module starts on its first request:

```text
HAL (linux-run)                                   linux-run --audio-io
 module `audio` ── socket: requests, ring memfds ──▶ device queries, AUHAL units
                ── lifeline: a pipe's write end ──▶ (EOF when the HAL is gone)
 ring memfd ◀──────────── mapped by both ─────────▶ render and capture callbacks
```

- **Why.** On 2026-09-28 the host's coreaudiod stopped answering every
  client (a plain host program's `AudioObjectGetPropertyData` hangs too). Its
  log repeats `Monitor::BeginWriteOperation: still waiting ... RIP: 1 RP: 0
  WIP: 0 WP: 1`: a property change (a write operation) waits for a read
  operation in progress that never ends, and the change is dispatched to a
  client whose notification worker never started (`HALS_Client.8317
  (worker not started)`). In the minutes before, the HAL process was
  restarted every 5 to 13 seconds (netd aborted, which restarts zygote,
  which restarts audioserver, which restarts the HAL; and the host's
  security agent killed processes, #232), each time querying the devices and
  opening streams, and each start asked TCC for microphone access. The
  hang began 8 seconds after one of those TCC requests. The most likely
  cause is a HAL process that was SIGKILLed while CoreAudio was setting up
  its client (or an input unit waiting for TCC): coreaudiod kept the half
  made client, and waits for it forever. A SIGKILL cannot be caught, so no
  stop or close path in the HAL process can prevent that; only a process
  that is not killed abruptly can.
- **Lifetime.** The CoreAudio process is the HAL process's child, in a
  process group of its own (so a SIGKILL of the service's group does not
  reach it), and ignores SIGINT and SIGHUP. When the HAL process goes,
  however it goes, the kernel closes its ends of the socket and the
  lifeline. The CoreAudio process then stops, uninitializes and disposes
  every unit (in that order), waits for input units still being set up,
  and exits. If CoreAudio itself does not return, it exits 10 seconds after
  the lifeline closed.
- **Input setup is deferred to the first start.** Opening an input stream
  only maps its ring; the unit (and with it the TCC request) is made on the
  stream's first `start`, on a thread of its own. audioserver opens the
  input at boot without starting it, so a boot makes no TCC request.
- **The module still maps each ring**, for the null sink below.

## Without CoreAudio

Every request to the CoreAudio process has a timeout: 3 seconds for the
device query and opens (the first request also starts the process, and it
is the first contact with coreaudiod), 2 seconds for start, stop and close.
A request that is not answered in time gives the process up for the rest
of the HAL process's life (no retry yet, #237): the module closes the
socket and the lifeline (the process tears down whenever CoreAudio lets
it), and

- the devices are a stereo 48 kHz output named "Null output" with a
  480-frame buffer, and no input, so audioserver's policy has its speaker
  and takes its no-microphone path;
- every stream, open or opened later, plays into the **null sink**: a
  thread that runs the render callback's own ring logic at the stream's
  rate (10 ms periods against CLOCK_MONOTONIC), so output is consumed,
  the position stamps advance as a device's would, xruns are counted, and
  AudioFlinger's threads keep their pace. An input stream on the null sink
  captures silence at its rate.
- A stream that was on the CoreAudio process gets `Ring::detached` set
  first: from then on its callbacks (should they still run) leave the ring
  alone, so the ring keeps one consumer.

Returning errors instead was rejected: audioserver treats a failed open of
the primary output as a missing module and retries it, and system_server's
`AudioService` then waits on audioserver. The null sink keeps the boot and
every app's audio path on their normal course.

## Verified (2026-09-28, MacBook Pro, M2 Pro)

`crates/aim-linux-abi/tests/audio.rs` (24 s) boots `guest-init --only
logd,servicemanager,hwservicemanager,system_suspend,vendor.audio-hal-aidl,audioserver`.
Services get /dev/null as stdio, so the test reads the HAL's messages from
logd (`logcat -s`); the host module's own lines are in the service's log
file, and the test fails if they show the null sink. The boot is stopped
when the test ends, passed or failed. Re-run with CoreAudio in its own
process (P3b): same results as below. Every test signal is at -90 dBFS: inaudible, checked through the render
callback's counters and peak meter, not by ear.

- **audioserver**, original: loads `libaudiohal@aidl` for both factories
  (`EffectsFactoryHalAidl with 0 nonProxyEffects`), takes its policy
  configuration from the HAL, sets the patch primary output to Speaker and
  opens the primary output (float, 48 kHz, 528 frames; FastMixer with a
  1056-frame normal sink) and briefly the input. Its threads' `standby`
  commands round-trip through the command and reply FMQs with the original
  libfmq.
- **Playback through the HAL** (`audio-hal-check`, the way libaudiohal
  drives it: mix config, the device's initial config, patch, stream at the
  patch's minimum buffer): 96,096 frames in 2,012 ms; 214 render callbacks
  of 2 µs host time each; peak -90.0 dBFS (the tone's own level); no
  underruns while active; the position ends at exactly the frames written
  after `drain`.
  - Write to render callback: 18 ms average, 21 ms maximum.
  - Write to speaker (from the reply's position and time): 54 ms, of which
    the device's own latency is 834 frames (17 ms) plus its 512-frame
    buffer.
  - CPU, 12 s of playback: the HAL process 1.0 % of one core, the client
    0.4 %.
- **Recording through the HAL**: 48,048 frames in 1,009 ms, silence. The
  test runs without microphone permission (macOS reports "not determined"
  for the terminal, and asking needs the user), so no capture callback ran;
  the stream delivered paced silence as designed. The capture path's ring
  logic is covered by a loopback test device in
  `crates/aim-host-audio` (`loopback_moves_frames_through_both_callbacks`:
  what the output callback renders is captured back into an input ring,
  frame-exact, including overflow). A microphone test is in the same crate,
  ignored by default because it asks for access.
- **Not yet reached: AAudio through AudioFlinger.** The test's NDK program
  (`tests/fixtures/audio_tone.c`, the original libaaudio) needs
  `media.audio_flinger`, which audioserver registers only after
  AudioPolicyService's construction. That waits, with no timeout, for
  system_server's `activity` service (the UID observer) and then
  `sensor_privacy`, so it follows P3. After that, AudioFlinger's client
  buffers need `/dev/ashmem` (libcutils uses memfd only with
  `sys.use_memfd`), which the syscall layer does not provide yet. The test
  runs the program once `media.audio_flinger` is registered.

Also seen: audioserver waits 5 s each for `media.metrics` (absent, it goes
on without) and needs `system_suspend` for its wake lock. hwservicemanager
must run, since audioserver configures the HIDL thread pool and waits for
`hwservicemanager.ready`.

## Host module `audio` (id 7)

| Function | Argument block | Result |
| --- | --- | --- |
| `FN_DEVICES` | `Devices` (out): default output and input: rate, channels, buffer, latency, name; the null devices when CoreAudio does not answer | 0 |
| `FN_OPEN` | `Open`: direction, format (PCM 16 or float), rate, channels, the ring memfd and its length; out: handle | 0, `-ENODEV`, `-EINVAL` |
| `FN_START`, `FN_STOP`, `FN_CLOSE` | `Stream`: handle | 0 or `-EINVAL` |

The module maps the ring from the guest's fd (the syscall layer's
descriptors are the process's own) and passes the fd on to the CoreAudio
process, which maps it too; neither keeps a guest pointer. Output uses
the DefaultOutput unit; input the HAL output unit with input enabled on the
default input device. Stamps convert CoreAudio's host time (mach absolute
time) to the guest's CLOCK_MONOTONIC, which is the host's.
