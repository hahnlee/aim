# Audio HAL (ADR 0012, P5)

The original audioserver (AudioFlinger and AudioPolicyService) plays and
records through our vendor audio HAL, which plays and records through
CoreAudio. It is the new runtime's form of #5's option B; the old stack's
interim path (option A) stays until the switch in P6.

| Piece | Where |
| --- | --- |
| HAL service: `IModule/default`, `IConfig/default` (audio.core V3) and `IFactory/default` (audio.effect V3) in one process | `hal/audio`, `/vendor/bin/hw/android.hardware.audio.service-aidl.darwin` |
| FMQs laid out as libfmq's (both sides) | `hal/fmq` (`darwin-fmq`) |
| Host module `audio` (id 7): AUHAL units and the shared ring | `crates/darwin-host-audio`, contract in `darwin_hostcall::audio` |
| `.rc` (service `vendor.audio-hal-aidl`) and vintf fragment | `hal/audio/*.rc`, `hal/audio/*.xml`, placed by `image/overlay.toml` |
| HAL test client (not in the image) | `hal/audio/check` (`audio-hal-check`) |
| Tests | `crates/darwin-host-audio` (unit, device), `crates/darwin-linux-abi/tests/audio.rs` (boot) |

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

## The primary module

Built from the Mac's default devices at start-up (`FN_DEVICES`), in the
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
audioserver                  HAL process (linux-run)                    CoreAudio I/O thread
libaudiohal ── data FMQ ──>  worker: burst → ring write (blocks if full)
            ── command FMQ ─>        ring: memfd, lock-free SPSC   ──>  render callback
            <─ reply FMQ ───         reply: position, latency            (never waits)
```

- **FMQs.** `darwin-fmq` lays queues out as libfmq's `MessageQueueBase`
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
  maps too (`darwin_hostcall::audio::Ring`): single producer, single
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
- **Input.** The input unit is set up on a thread of its own: its first
  use asks macOS for microphone access and waits for the user's answer
  (TCC), which the HAL must not wait for. Until the first capture callback
  the stream delivers silence at the capture rate, as from a muted
  microphone, so AudioFlinger's record thread keeps its pace.
- **Stats.** On `standby` and `close` the worker logs what the callback
  saw: frames, callbacks and their host time, xruns, peak (dBFS) and the
  write-to-callback delay.

## Verified (2026-09-28, MacBook Pro, M2 Pro)

`crates/darwin-linux-abi/tests/audio.rs` (20 s) boots `guest-init --only
servicemanager,hwservicemanager,system_suspend,vendor.audio-hal-aidl,audioserver`.
Every test signal is at -90 dBFS: inaudible, checked through the render
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
  `crates/darwin-host-audio` (`loopback_moves_frames_through_both_callbacks`:
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
| `FN_DEVICES` | `Devices` (out): default output and input: rate, channels, buffer, latency, name | 0 |
| `FN_OPEN` | `Open`: direction, format (PCM 16 or float), rate, channels, the ring memfd and its length; out: handle | 0, `-ENODEV`, `-EINVAL` |
| `FN_START`, `FN_STOP`, `FN_CLOSE` | `Stream`: handle | 0 or `-EINVAL` |

The host maps the ring from the guest's fd (the syscall layer's
descriptors are the process's own) and keeps no guest pointer. Output uses
the DefaultOutput unit; input the HAL output unit with input enabled on the
default input device. Stamps convert CoreAudio's host time (mach absolute
time) to the guest's CLOCK_MONOTONIC, which is the host's.
