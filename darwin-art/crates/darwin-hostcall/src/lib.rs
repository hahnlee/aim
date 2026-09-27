//! Host-call ABI: how guest code reaches host implementations (ADR 0012,
//! boundary 2; `docs/host-call.md`).
//!
//! A guest component (a vendor HAL service built for `aarch64-linux-android`,
//! running on the Linux syscall layer) calls a host module the way a Wine
//! `.drv` calls its Unix side:
//!
//! ```text
//! x8 = SYSCALL_NR, x0 = module, x1 = function, x2 = args, x3 = args length
//! svc #0                                  -> x0 = result (>= 0) or -errno
//! ```
//!
//! - `args` points to the function's `#[repr(C)]` argument block in guest
//!   memory, which is host memory too: guest and host share one address
//!   space, so nothing is copied. The host checks that `len` is exactly the
//!   block's size before touching it.
//! - Register semantics are those of an AAPCS64 function call, not of a
//!   Linux syscall: x0 carries the result; x1-x17, v0-v7, v16-v31, the upper
//!   halves of v8-v15 and NZCV may be clobbered; everything else is kept.
//!   [`guest::call`] declares exactly that.
//! - Function 0 of every module ([`FN_VERSION`]) returns the module's
//!   version; module [`module::CORE`] reports [`ABI_VERSION`]. An unknown
//!   module or function returns `-ENOSYS`, which is also what a real Linux
//!   kernel answers for [`SYSCALL_NR`], so a guest can probe for the host.
//!
//! This crate is `no_std` and holds only the contract: numbers, argument
//! layouts, the guest-side call ([`guest`], aarch64 Linux/Android only) and
//! the shape of a host module ([`HostModule`]).

#![no_std]

/// Version of the calling convention itself (register use, [`FN_VERSION`],
/// error convention). Bumped only for incompatible changes.
pub const ABI_VERSION: u32 = 1;

/// The Linux syscall number that carries a host call: `"HC" << 16`, far
/// above any Linux arm64 syscall.
pub const SYSCALL_NR: u64 = 0x4843_0000;

/// Function 0 of every module: returns the module's version.
pub const FN_VERSION: u32 = 0;

/// Linux errno values used by the host-call layer and its modules.
pub mod errno {
    pub const EBADF: i32 = 9;
    pub const EAGAIN: i32 = 11;
    pub const EBUSY: i32 = 16;
    pub const ENODEV: i32 = 19;
    pub const EINVAL: i32 = 22;
    pub const ENOSYS: i32 = 38;
    pub const EMSGSIZE: i32 = 90;
    pub const ENOTCONN: i32 = 107;
    pub const ECONNREFUSED: i32 = 111;
}

/// Module ids. They are fixed like syscall numbers: never reused, never
/// renumbered. A module evolves by bumping its version and adding
/// functions; an argument block never changes layout once shipped.
pub mod module {
    /// The host-call layer itself; its version is [`crate::ABI_VERSION`].
    pub const CORE: u32 = 0;
    /// Battery and power supply (the health HAL's host side).
    pub const HEALTH: u32 = 1;
    /// EGL and OpenGL ES over the host's ANGLE (the GLES driver's host side).
    pub const GPU: u32 = 2;
    /// The display server's window (the composer HAL's host side).
    pub const DISPLAY: u32 = 3;
    /// Thermal state and temperatures (the thermal HAL's host side).
    pub const THERMAL: u32 = 4;
    /// Ambient light and lid angle (the sensors HAL's host side).
    pub const SENSORS: u32 = 5;
    /// CoreLocation fixes (the GNSS HAL's host side).
    pub const LOCATION: u32 = 6;
    /// Audio streams over CoreAudio (the audio HAL's host side).
    pub const AUDIO: u32 = 7;
    /// A virtual HCI controller over CoreBluetooth (the Bluetooth HAL's
    /// host side).
    pub const BLUETOOTH: u32 = 8;
}

/// Module [`module::HEALTH`]: the host's battery, for
/// `android.hardware.health`.
pub mod health {
    pub const VERSION: u32 = 1;

    /// Fill a [`Battery`]. Returns 0.
    pub const FN_BATTERY: u32 = 1;

    /// `android.hardware.health.BatteryStatus` values.
    pub mod status {
        pub const UNKNOWN: u32 = 1;
        pub const CHARGING: u32 = 2;
        pub const DISCHARGING: u32 = 3;
        pub const NOT_CHARGING: u32 = 4;
        pub const FULL: u32 = 5;
    }

    /// `android.hardware.health.BatteryHealth` values.
    pub mod condition {
        pub const UNKNOWN: u32 = 1;
        pub const GOOD: u32 = 2;
        pub const UNSPECIFIED_FAILURE: u32 = 6;
        pub const FAIR: u32 = 8;
        pub const NOT_AVAILABLE: u32 = 11;
    }

    /// Argument block of [`FN_BATTERY`] (output only). Units and signs
    /// follow `android.hardware.health.HealthInfo`; a value the host does
    /// not know is 0, and -1 for the two times.
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Battery {
        /// 0 when the host has no battery; the battery fields are then 0.
        pub present: u32,
        /// External power is connected.
        pub ac_online: u32,
        /// One of [`status`].
        pub status: u32,
        /// One of [`condition`].
        pub health: u32,
        pub level_percent: i32,
        pub voltage_millivolts: i32,
        /// Positive while charging, negative while discharging.
        pub current_microamps: i32,
        pub temperature_tenths_celsius: i32,
        pub cycle_count: i32,
        pub charge_counter_uah: i32,
        pub full_charge_uah: i32,
        pub full_charge_design_uah: i32,
        pub time_to_full_seconds: i64,
        pub time_to_empty_seconds: i64,
    }

    const _: () = assert!(core::mem::size_of::<Battery>() == 64);
}

/// Module [`module::GPU`]: EGL and OpenGL ES over the host's ANGLE on
/// Metal, for the guest GLES driver (`docs/gles-driver.md`).
pub mod gpu {
    pub const VERSION: u32 = 1;

    /// Load ANGLE and resolve the forwarded entry points ([`Init`]).
    /// Returns 0, `-ENODEV` when the host has no GPU libraries, or
    /// `-EINVAL` when the guest's table is not the host's.
    pub const FN_INIT: u32 = 1;
    /// Wrap guest memory as a linear Metal texture and import it into ANGLE
    /// as an `EGLImage` ([`ImportBuffer`]). Returns 0 or `-EINVAL`.
    pub const FN_IMPORT_BUFFER: u32 = 2;
    /// Copy the current context's draw surface into an `EGLImage`, top row
    /// first, and wait for the GPU ([`Present`]). Returns 0 or `-EINVAL`.
    pub const FN_PRESENT: u32 = 3;
    /// Forwarded entry point `i` of the generated table
    /// (`tools/gen-gpu-thunks.py`) is function `FN_TABLE_BASE + i`. Its
    /// argument block is the callee's register image, 8 bytes per value:
    /// the x registers, then the d registers, then the stack words in the
    /// Apple arm64 layout. The call returns the callee's x0 unchanged (0 for
    /// an entry point the host could not resolve).
    pub const FN_TABLE_BASE: u32 = 0x1000;

    /// Argument block of [`FN_INIT`].
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    pub struct Init {
        /// `TABLE_HASH` of the generated table.
        pub table_hash: u64,
        pub table_len: u64,
        /// Guest bitmap of `table_len` bits, one u64 per 64 entries: the
        /// host sets bit `i` when it resolved entry `i`.
        pub resolved: u64,
        pub resolved_words: u64,
    }

    /// Argument block of [`FN_IMPORT_BUFFER`].
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    pub struct ImportBuffer {
        /// The host `EGLDisplay`.
        pub display: u64,
        /// Page-aligned guest mapping of the whole buffer, pixels at 0.
        pub address: u64,
        /// Length of the mapping, a multiple of the page size.
        pub length: u64,
        pub width: u32,
        pub height: u32,
        pub stride_bytes: u32,
        /// `PixelFormat` of the buffer.
        pub format: i32,
        /// Out: the `EGLImage`.
        pub image: u64,
    }

    /// Argument block of [`FN_PRESENT`].
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    pub struct Present {
        /// Destination `EGLImage` (from [`FN_IMPORT_BUFFER`]).
        pub image: u64,
        pub src_width: u32,
        pub src_height: u32,
        pub dst_width: u32,
        pub dst_height: u32,
    }

    const _: () = assert!(core::mem::size_of::<Init>() == 32);
    const _: () = assert!(core::mem::size_of::<ImportBuffer>() == 48);
    const _: () = assert!(core::mem::size_of::<Present>() == 24);
}

/// Module [`module::DISPLAY`]: the macOS window that shows a display, for
/// the composer HAL (`docs/composer.md`).
///
/// A display is served by the display server (`darwin-display`, named by
/// `linux-run --display`), which owns the window and its display link. The
/// composer shows graphics buffers there without copying them: the server
/// maps each buffer's memory once and presents it with one GPU pass.
/// Asynchronous events (vsync) arrive as [`Event`] records on a descriptor
/// the guest reads, since host code never calls guest code.
pub mod display {
    pub const VERSION: u32 = 1;

    /// Connect to the display server and describe display
    /// [`Connect::display`]. Returns a new guest fd that yields [`Event`]
    /// records, `-ENODEV` without a display server, or `-ECONNREFUSED`
    /// when it is not running. A second connect replaces the first.
    pub const FN_CONNECT: u32 = 1;
    /// Give the server a graphics buffer ([`Import`]). Returns 0, or
    /// `-EBADF`/`-EINVAL` for a bad fd or layout, `-ENOTCONN` before
    /// [`FN_CONNECT`].
    pub const FN_IMPORT: u32 = 2;
    /// Show an imported buffer ([`Buffer`]). Returns 0 or `-ENOTCONN`.
    pub const FN_PRESENT: u32 = 3;
    /// Forget an imported buffer ([`Buffer`]). Returns 0 or `-ENOTCONN`.
    pub const FN_RELEASE: u32 = 4;
    /// Start or stop [`event::VSYNC`] records ([`SetVsync`]). Returns 0 or
    /// `-ENOTCONN`.
    pub const FN_SET_VSYNC: u32 = 5;

    /// Argument block of [`FN_CONNECT`].
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Connect {
        /// In: display index (0 is the main display).
        pub display: u32,
        /// Out: the display mode, in pixels.
        pub width: u32,
        pub height: u32,
        /// Out: dots per inch times 1000.
        pub dpi_x_milli: u32,
        pub dpi_y_milli: u32,
        pub _reserved: u32,
        /// Out: nominal time between vsyncs.
        pub vsync_period_ns: u64,
    }

    /// Argument block of [`FN_IMPORT`]. The buffer is `length` bytes of
    /// `fd` from offset 0 (docs/graphics-buffers.md): rows of
    /// `stride_bytes`, the top row first.
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Import {
        /// The buffer's guest fd; the server keeps its own reference.
        pub fd: i32,
        /// `PixelFormat` of the buffer.
        pub format: i32,
        pub width: u32,
        pub height: u32,
        pub stride_bytes: u32,
        pub _reserved: u32,
        /// A multiple of the page size, covering all the pixels.
        pub length: u64,
        /// The caller's name for the buffer in later calls.
        pub id: u64,
    }

    /// Argument block of [`FN_PRESENT`] and [`FN_RELEASE`].
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Buffer {
        pub id: u64,
    }

    /// Argument block of [`FN_SET_VSYNC`].
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct SetVsync {
        pub enabled: u32,
    }

    /// [`Event::kind`] values.
    pub mod event {
        /// A vsync: [`super::Event::timestamp_ns`] is when it happened and
        /// [`super::Event::period_ns`] the time to the next one.
        pub const VSYNC: u32 = 1;
    }

    /// A record read from the fd [`FN_CONNECT`] returns. Times are the
    /// guest's `CLOCK_MONOTONIC`.
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Event {
        /// One of [`event`].
        pub kind: u32,
        pub _reserved: u32,
        pub timestamp_ns: i64,
        pub period_ns: i64,
        /// When the server sent the record.
        pub sent_ns: i64,
    }

    const _: () = assert!(core::mem::size_of::<Connect>() == 32);
    const _: () = assert!(core::mem::size_of::<Import>() == 40);
    const _: () = assert!(core::mem::size_of::<Buffer>() == 8);
    const _: () = assert!(core::mem::size_of::<SetVsync>() == 4);
    const _: () = assert!(core::mem::size_of::<Event>() == 32);
}

/// Module [`module::THERMAL`]: the host's thermal state and temperatures,
/// for `android.hardware.thermal`.
pub mod thermal {
    pub const VERSION: u32 = 1;

    /// Fill a [`Thermal`]. Returns 0.
    pub const FN_READ: u32 = 1;

    /// `NSProcessInfoThermalState` values.
    pub mod state {
        pub const NOMINAL: u32 = 0;
        pub const FAIR: u32 = 1;
        pub const SERIOUS: u32 = 2;
        pub const CRITICAL: u32 = 3;
    }

    /// Argument block of [`FN_READ`] (output only). A temperature the host
    /// cannot read is NaN.
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub struct Thermal {
        /// One of [`state`].
        pub state: u32,
        /// The hottest CPU die sensor, in degrees Celsius.
        pub cpu_celsius: f32,
        /// The battery (mean of its gauge sensors), in degrees Celsius.
        pub battery_celsius: f32,
    }

    const _: () = assert!(core::mem::size_of::<Thermal>() == 12);
}

/// Module [`module::SENSORS`]: the host's ambient light sensor and lid
/// angle, for `android.hardware.sensors`.
pub mod sensors {
    pub const VERSION: u32 = 1;

    /// Fill a [`Readings`]. Returns 0.
    pub const FN_READ: u32 = 1;

    /// Bits of [`Readings::present`].
    pub mod present {
        pub const LIGHT: u32 = 1 << 0;
        pub const HINGE: u32 = 1 << 1;
    }

    /// Argument block of [`FN_READ`] (output only). A reading is valid when
    /// its bit is set in `present`.
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub struct Readings {
        /// Bits of [`present`]: the sensors this host has.
        pub present: u32,
        pub light_lux: f32,
        /// Lid opening, 0 (closed) to 360 degrees.
        pub hinge_degrees: f32,
    }

    const _: () = assert!(core::mem::size_of::<Readings>() == 12);
}

/// Module [`module::LOCATION`]: CoreLocation fixes, for
/// `android.hardware.gnss`.
pub mod location {
    pub const VERSION: u32 = 1;

    /// Start location updates (no argument block). Returns 0.
    pub const FN_START: u32 = 1;
    /// Stop location updates (no argument block). Returns 0.
    pub const FN_STOP: u32 = 2;
    /// Fill a [`Fix`] with the latest location. Returns 0.
    pub const FN_READ: u32 = 3;

    /// Whether the host lets us read the location.
    pub mod authorization {
        /// The user has not answered the prompt yet.
        pub const NOT_DETERMINED: u32 = 0;
        pub const RESTRICTED: u32 = 1;
        pub const DENIED: u32 = 2;
        pub const AUTHORIZED: u32 = 3;
        /// Location Services are off system-wide.
        pub const SERVICES_OFF: u32 = 4;
    }

    /// Argument block of [`FN_READ`] (output only). Units follow
    /// CoreLocation: degrees, meters, meters per second; an accuracy below
    /// 0 marks its value as invalid.
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub struct Fix {
        /// One of [`authorization`].
        pub authorization: u32,
        /// 1 when the fields below hold a location.
        pub valid: u32,
        pub latitude: f64,
        pub longitude: f64,
        pub altitude: f64,
        pub horizontal_accuracy: f64,
        pub vertical_accuracy: f64,
        pub speed: f64,
        pub speed_accuracy: f64,
        /// Course over ground, degrees clockwise from true north.
        pub course: f64,
        pub course_accuracy: f64,
        /// Time since the fix, in nanoseconds.
        pub age_ns: u64,
        /// Time of the fix, in milliseconds since the Unix epoch.
        pub unix_ms: i64,
    }

    const _: () = assert!(core::mem::size_of::<Fix>() == 96);
}

/// Module [`module::AUDIO`]: PCM streams on the Mac's default output and
/// input devices, for `android.hardware.audio.core` (`docs/audio.md`).
///
/// A stream moves frames through a [`Ring`] in a memfd that the guest
/// creates and maps; the host maps the same file. The ring is single
/// producer, single consumer and lock-free: for output the guest produces
/// and the device callback consumes, for input the reverse. The device
/// callback never waits for the guest: a short ring plays silence (and
/// counts the frames in [`Ring::xruns`]), a full one drops captured frames.
pub mod audio {
    use core::sync::atomic::{AtomicI64, AtomicU32, AtomicU64};

    pub const VERSION: u32 = 1;

    /// Describe the default devices ([`Devices`]). Returns 0.
    pub const FN_DEVICES: u32 = 1;
    /// Open a stream on a ring ([`Open`]). Returns 0, `-ENODEV` when there
    /// is no such device, or `-EINVAL` for a bad format or ring.
    pub const FN_OPEN: u32 = 2;
    /// Start the device callback ([`Stream`]). Returns 0 or `-EINVAL`.
    pub const FN_START: u32 = 3;
    /// Stop the device callback; the ring is left as it is ([`Stream`]).
    pub const FN_STOP: u32 = 4;
    /// Stop and release a stream and its mapping ([`Stream`]).
    pub const FN_CLOSE: u32 = 5;

    pub const DIRECTION_OUTPUT: u32 = 0;
    pub const DIRECTION_INPUT: u32 = 1;

    /// Interleaved signed 16-bit PCM.
    pub const FORMAT_PCM_16: u32 = 1;
    /// Interleaved 32-bit float PCM.
    pub const FORMAT_FLOAT: u32 = 2;

    /// A default device as macOS reports it.
    #[repr(C)]
    #[derive(Clone, Copy, Debug)]
    pub struct Device {
        /// 0 when the Mac has no such default device; the rest is then 0.
        pub present: u32,
        pub sample_rate: u32,
        pub channels: u32,
        /// The device's I/O buffer size.
        pub buffer_frames: u32,
        /// Device and stream latency plus the safety offset.
        pub latency_frames: u32,
        pub reserved: u32,
        /// UTF-8, NUL-padded.
        pub name: [u8; 64],
    }

    impl Default for Device {
        fn default() -> Self {
            Self {
                present: 0,
                sample_rate: 0,
                channels: 0,
                buffer_frames: 0,
                latency_frames: 0,
                reserved: 0,
                name: [0; 64],
            }
        }
    }

    /// Argument block of [`FN_DEVICES`] (output only).
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    pub struct Devices {
        pub output: Device,
        pub input: Device,
    }

    /// Argument block of [`FN_OPEN`].
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    pub struct Open {
        /// [`DIRECTION_OUTPUT`] or [`DIRECTION_INPUT`].
        pub direction: u32,
        /// [`FORMAT_PCM_16`] or [`FORMAT_FLOAT`].
        pub format: u32,
        pub sample_rate: u32,
        pub channels: u32,
        /// The ring's memfd; the host maps it and does not keep the fd.
        pub fd: i32,
        pub reserved: u32,
        /// Length of the file: [`RING_DATA_OFFSET`] plus the ring's frames.
        pub length: u64,
        /// Out: the stream handle.
        pub stream: u64,
    }

    /// Argument block of [`FN_START`], [`FN_STOP`] and [`FN_CLOSE`].
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    pub struct Stream {
        pub stream: u64,
    }

    /// Frames start this far into the ring's file.
    pub const RING_DATA_OFFSET: usize = 256;

    /// Header at offset 0 of the ring's file. Positions are frame counts
    /// since the ring was opened; frame `n` is at data offset
    /// `(n % capacity_frames) * frame_bytes`. The guest fills the first two
    /// fields before [`FN_OPEN`]; everything else is atomic.
    #[repr(C)]
    #[derive(Debug, Default)]
    pub struct Ring {
        pub capacity_frames: u32,
        pub frame_bytes: u32,
        /// Frames produced (by the guest for output, the host for input).
        pub write: AtomicU64,
        /// Frames consumed.
        pub read: AtomicU64,
        /// Output: frames the device played as silence because the ring ran
        /// short. Input: captured frames dropped because it was full.
        pub xruns: AtomicU64,
        /// Device callbacks run, and host time spent in them.
        pub callbacks: AtomicU64,
        pub callback_ns: AtomicU64,
        /// Seqlock over `stamp_frames` and `stamp_ns`: odd while the host
        /// writes them.
        pub stamp_seq: AtomicU32,
        /// Device and stream latency the stamps include.
        pub device_latency_frames: AtomicU32,
        /// Ring position `stamp_frames` reaches the speaker (output) or was
        /// captured at the microphone (input) at CLOCK_MONOTONIC
        /// `stamp_ns`.
        pub stamp_frames: AtomicU64,
        pub stamp_ns: AtomicI64,
        /// Largest |sample| the device callback moved (f32 bits, full scale
        /// 1.0), until the guest resets it.
        pub peak_bits: AtomicU32,
        pub reserved: u32,
        /// Latency probe, output only. The guest stores the CLOCK_MONOTONIC
        /// time of a write in `mark_ns`, then its first frame plus one in
        /// `mark` (only while `mark` is 0). The callback that consumes that
        /// frame adds its delay to the sums and clears `mark`.
        pub mark: AtomicU64,
        pub mark_ns: AtomicI64,
        pub latency_count: AtomicU64,
        pub latency_sum_ns: AtomicU64,
        pub latency_max_ns: AtomicU64,
    }

    const _: () = assert!(core::mem::size_of::<Device>() == 88);
    const _: () = assert!(core::mem::size_of::<Devices>() == 176);
    const _: () = assert!(core::mem::size_of::<Open>() == 40);
    const _: () = assert!(core::mem::size_of::<Stream>() == 8);
    const _: () = assert!(core::mem::size_of::<Ring>() == 120);
    const _: () = assert!(core::mem::size_of::<Ring>() <= RING_DATA_OFFSET);
}

/// Module [`module::BLUETOOTH`]: a virtual HCI controller over
/// CoreBluetooth, for `android.hardware.bluetooth` (`docs/bluetooth.md`).
///
/// Packets are H4 payloads without the type byte, as `IBluetoothHci`
/// carries them. Controller-to-host packets queue in the host; the fd
/// [`FN_OPEN`] returns becomes readable when the queue is not empty, and
/// the guest drains it with [`FN_RECV`]. Host code never calls the guest.
pub mod bluetooth {
    pub const VERSION: u32 = 1;

    /// Open the controller (powered off until HCI_Reset). Takes no argument
    /// block. Returns the wake fd (a pipe's read end, close-on-exec,
    /// non-blocking): each byte in it means "the queue has packets". Returns
    /// `-EBUSY` when the controller is already open.
    pub const FN_OPEN: u32 = 1;
    /// Hand a host-to-controller packet to the controller ([`Packet`], the
    /// data read). Returns 0, `-EINVAL` for an unknown kind or malformed
    /// packet, or `-ENODEV` when the controller is not open.
    pub const FN_SEND: u32 = 2;
    /// Take the next controller-to-host packet ([`Packet`], the data
    /// written, `len` set to its length). Returns 0, `-EAGAIN` when the
    /// queue is empty, `-EMSGSIZE` when `capacity` is too small (the packet
    /// stays queued), or `-ENODEV`.
    pub const FN_RECV: u32 = 3;
    /// Close the controller: drop every link and scan, and the queue. Takes
    /// no argument block. Returns 0.
    pub const FN_CLOSE: u32 = 4;

    /// Packet kinds, the H4 type numbers.
    pub mod kind {
        pub const COMMAND: u32 = 1;
        pub const ACL: u32 = 2;
        pub const SCO: u32 = 3;
        pub const EVENT: u32 = 4;
        pub const ISO: u32 = 5;
    }

    /// The largest packet either way: an ACL packet of 4 header bytes and
    /// the controller's 1021-byte buffers, or an event of 2 + 255.
    pub const MAX_PACKET: usize = 1028;

    /// Argument block of [`FN_SEND`] and [`FN_RECV`].
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    pub struct Packet {
        /// One of [`kind`]; set by the host in [`FN_RECV`].
        pub kind: u32,
        /// Bytes in `data`; set by the host in [`FN_RECV`].
        pub len: u32,
        /// Guest buffer.
        pub data: u64,
        /// Size of the buffer at `data` ([`FN_RECV`] only).
        pub capacity: u64,
    }

    const _: () = assert!(core::mem::size_of::<Packet>() == 24);
}

/// A host module, as linked into the syscall layer's registry.
pub struct HostModule {
    pub id: u32,
    pub name: &'static str,
    /// Returned by [`FN_VERSION`], which the registry answers itself.
    pub version: u32,
    /// Runs function `func` (never [`FN_VERSION`]) with the guest's
    /// argument block. Returns a value >= 0 or a negative Linux errno.
    ///
    /// # Safety
    /// `args` is a guest address the host has not validated; `len` is the
    /// length the guest claims for it. The function must check `len` (see
    /// [`args_mut`]) before touching the block.
    pub call: unsafe fn(func: u32, args: u64, len: u64) -> i64,
}

/// The argument block of type `T` at guest address `args`, if `len` is its
/// exact size and `args` is non-null and aligned; `-EINVAL` otherwise.
///
/// # Safety
/// When the checks pass, `args` must point to `len` bytes of writable guest
/// memory not aliased for the returned lifetime (the calling thread's
/// argument block during its host call).
pub unsafe fn args_mut<'a, T>(args: u64, len: u64) -> Result<&'a mut T, i64> {
    let invalid = Err(-(errno::EINVAL as i64));
    if len != core::mem::size_of::<T>() as u64
        || args == 0
        || args % core::mem::align_of::<T>() as u64 != 0
    {
        return invalid;
    }
    // SAFETY: checked above; the caller vouches for the memory.
    Ok(unsafe { &mut *(args as *mut T) })
}

/// Guest side: the call instruction and typed wrappers.
#[cfg(all(
    target_arch = "aarch64",
    any(target_os = "android", target_os = "linux")
))]
pub mod guest {
    use super::*;

    /// A negative Linux errno from a host call.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Errno(pub i32);

    /// Raw host call.
    ///
    /// # Safety
    /// `args` must point to `len` bytes laid out as `module`'s function
    /// `func` expects.
    #[inline]
    pub unsafe fn call(module: u32, func: u32, args: *mut u8, len: usize) -> i64 {
        let ret: i64;
        // SAFETY: the host-call syscall, with AAPCS64 call clobbers.
        unsafe {
            core::arch::asm!(
                "svc #0",
                in("x8") SYSCALL_NR,
                inlateout("x0") module as u64 => ret,
                in("x1") func as u64,
                in("x2") args,
                in("x3") len,
                clobber_abi("C"),
            );
        }
        ret
    }

    fn check(ret: i64) -> Result<i64, Errno> {
        if ret < 0 {
            Err(Errno(-ret as i32))
        } else {
            Ok(ret)
        }
    }

    fn call_with<T>(module: u32, func: u32, args: &mut T) -> Result<i64, Errno> {
        // SAFETY: T is the argument block the function documents.
        check(unsafe {
            call(
                module,
                func,
                (args as *mut T).cast(),
                core::mem::size_of::<T>(),
            )
        })
    }

    /// Version of `module`; `Err(Errno(ENOSYS))` when the module (or the
    /// host-call layer) is absent.
    pub fn version(module: u32) -> Result<u32, Errno> {
        // SAFETY: FN_VERSION takes no argument block.
        check(unsafe { call(module, FN_VERSION, core::ptr::null_mut(), 0) }).map(|v| v as u32)
    }

    /// The host's battery.
    pub fn battery() -> Result<health::Battery, Errno> {
        let mut b = health::Battery::default();
        call_with(module::HEALTH, health::FN_BATTERY, &mut b)?;
        Ok(b)
    }

    /// Open the Bluetooth controller; returns its wake fd.
    pub fn bluetooth_open() -> Result<i32, Errno> {
        // SAFETY: FN_OPEN takes no argument block.
        check(unsafe {
            call(
                module::BLUETOOTH,
                bluetooth::FN_OPEN,
                core::ptr::null_mut(),
                0,
            )
        })
        .map(|fd| fd as i32)
    }

    /// Send a host-to-controller packet of `kind`.
    pub fn bluetooth_send(kind: u32, data: &[u8]) -> Result<(), Errno> {
        let mut p = bluetooth::Packet {
            kind,
            len: data.len() as u32,
            data: data.as_ptr() as u64,
            capacity: data.len() as u64,
        };
        call_with(module::BLUETOOTH, bluetooth::FN_SEND, &mut p).map(drop)
    }

    /// Take the next controller-to-host packet into `buf`: its kind and
    /// length, or `Err(Errno(EAGAIN))` when none is queued.
    pub fn bluetooth_recv(buf: &mut [u8]) -> Result<(u32, usize), Errno> {
        let mut p = bluetooth::Packet {
            data: buf.as_mut_ptr() as u64,
            capacity: buf.len() as u64,
            ..Default::default()
        };
        call_with(module::BLUETOOTH, bluetooth::FN_RECV, &mut p)?;
        Ok((p.kind, p.len as usize))
    }

    /// Close the Bluetooth controller.
    pub fn bluetooth_close() -> Result<(), Errno> {
        // SAFETY: FN_CLOSE takes no argument block.
        check(unsafe {
            call(
                module::BLUETOOTH,
                bluetooth::FN_CLOSE,
                core::ptr::null_mut(),
                0,
            )
        })
        .map(drop)
    }

    /// Load the host GPU libraries and resolve the forwarded entry points.
    pub fn gpu_init(args: &mut gpu::Init) -> Result<(), Errno> {
        call_with(module::GPU, gpu::FN_INIT, args).map(drop)
    }

    /// Import a mapped graphics buffer into ANGLE as an `EGLImage`.
    pub fn gpu_import_buffer(args: &mut gpu::ImportBuffer) -> Result<(), Errno> {
        call_with(module::GPU, gpu::FN_IMPORT_BUFFER, args).map(drop)
    }

    /// Copy the current draw surface into an imported buffer.
    pub fn gpu_present(args: &mut gpu::Present) -> Result<(), Errno> {
        call_with(module::GPU, gpu::FN_PRESENT, args).map(drop)
    }

    /// Connect to the display server; returns the event fd.
    pub fn display_connect(args: &mut display::Connect) -> Result<i32, Errno> {
        call_with(module::DISPLAY, display::FN_CONNECT, args).map(|fd| fd as i32)
    }

    /// Give the display server a graphics buffer.
    pub fn display_import(args: &mut display::Import) -> Result<(), Errno> {
        call_with(module::DISPLAY, display::FN_IMPORT, args).map(drop)
    }

    /// Show an imported buffer.
    pub fn display_present(id: u64) -> Result<(), Errno> {
        call_with(
            module::DISPLAY,
            display::FN_PRESENT,
            &mut display::Buffer { id },
        )
        .map(drop)
    }

    /// Forget an imported buffer.
    pub fn display_release(id: u64) -> Result<(), Errno> {
        call_with(
            module::DISPLAY,
            display::FN_RELEASE,
            &mut display::Buffer { id },
        )
        .map(drop)
    }

    /// Start or stop vsync events.
    pub fn display_set_vsync(enabled: bool) -> Result<(), Errno> {
        let mut args = display::SetVsync {
            enabled: enabled as u32,
        };
        call_with(module::DISPLAY, display::FN_SET_VSYNC, &mut args).map(drop)
    }

    /// The host's thermal state and temperatures.
    pub fn thermal() -> Result<thermal::Thermal, Errno> {
        let mut t = thermal::Thermal::default();
        call_with(module::THERMAL, thermal::FN_READ, &mut t)?;
        Ok(t)
    }

    /// The host's ambient light and lid angle.
    pub fn sensors() -> Result<sensors::Readings, Errno> {
        let mut r = sensors::Readings::default();
        call_with(module::SENSORS, sensors::FN_READ, &mut r)?;
        Ok(r)
    }

    /// Start (`true`) or stop host location updates.
    pub fn location_updates(on: bool) -> Result<(), Errno> {
        let func = if on {
            location::FN_START
        } else {
            location::FN_STOP
        };
        // SAFETY: FN_START and FN_STOP take no argument block.
        check(unsafe { call(module::LOCATION, func, core::ptr::null_mut(), 0) }).map(drop)
    }

    /// The latest host location.
    pub fn location() -> Result<location::Fix, Errno> {
        let mut f = location::Fix::default();
        call_with(module::LOCATION, location::FN_READ, &mut f)?;
        Ok(f)
    }

    /// The host's default audio devices.
    pub fn audio_devices() -> Result<audio::Devices, Errno> {
        let mut d = audio::Devices::default();
        call_with(module::AUDIO, audio::FN_DEVICES, &mut d)?;
        Ok(d)
    }

    /// Open an audio stream on a ring; returns its handle.
    pub fn audio_open(args: &mut audio::Open) -> Result<u64, Errno> {
        call_with(module::AUDIO, audio::FN_OPEN, args)?;
        Ok(args.stream)
    }

    /// [`audio::FN_START`], [`audio::FN_STOP`] or [`audio::FN_CLOSE`] on a
    /// stream.
    pub fn audio_stream(func: u32, stream: u64) -> Result<(), Errno> {
        call_with(module::AUDIO, func, &mut audio::Stream { stream }).map(drop)
    }

    /// Call forwarded entry point `id` with its register image.
    ///
    /// # Safety
    /// `regs` must hold `words` values laid out as that entry point's
    /// register image, and any pointers in it must be valid for the callee.
    #[inline(always)]
    pub unsafe fn gpu_forward(id: u32, regs: *mut u64, words: usize) -> i64 {
        // SAFETY: caller contract.
        unsafe { call_fn(module::GPU, gpu::FN_TABLE_BASE + id, regs.cast(), words * 8) }
    }

    /// [`call`] as an out-of-line C function. A caller then saves only the
    /// registers AAPCS64 makes it save, whereas inline assembly must also
    /// treat v8-v15 as clobbered (it cannot name their upper halves alone),
    /// which costs a small caller eight extra register saves.
    ///
    /// # Safety
    /// As for [`call`].
    #[inline(always)]
    pub unsafe fn call_fn(module: u32, func: u32, args: *mut u8, len: usize) -> i64 {
        unsafe extern "C" {
            fn darwin_hostcall(module: u32, func: u32, args: *mut u8, len: usize) -> i64;
        }
        // SAFETY: caller contract; the function is the host-call syscall.
        unsafe { darwin_hostcall(module, func, args, len) }
    }

    core::arch::global_asm!(
        ".pushsection .text.darwin_hostcall,\"ax\",@progbits",
        ".globl darwin_hostcall",
        ".hidden darwin_hostcall",
        ".type darwin_hostcall,@function",
        ".p2align 2",
        "darwin_hostcall:",
        "movz x8, #0x4843, lsl #16",
        "svc #0",
        "ret",
        ".size darwin_hostcall, . - darwin_hostcall",
        ".popsection",
    );
    const _: () = assert!(SYSCALL_NR == 0x4843 << 16);
}
