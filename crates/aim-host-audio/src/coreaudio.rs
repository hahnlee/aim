//! The CoreAudio and AudioToolbox declarations this module uses (from the
//! macOS SDK headers), and queries of the default devices.

use std::ffi::{c_char, c_void};

use aim_hostcall::audio::Device;

pub type OSStatus = i32;
pub type AudioObjectID = u32;
pub type AudioUnit = *mut c_void;
type AudioComponent = *mut c_void;
type CFStringRef = *const c_void;

pub const fn fourcc(s: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*s)
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct AudioStreamBasicDescription {
    pub sample_rate: f64,
    pub format_id: u32,
    pub format_flags: u32,
    pub bytes_per_packet: u32,
    pub frames_per_packet: u32,
    pub bytes_per_frame: u32,
    pub channels_per_frame: u32,
    pub bits_per_channel: u32,
    pub reserved: u32,
}

pub const FORMAT_LINEAR_PCM: u32 = fourcc(b"lpcm");
pub const FORMAT_FLAG_IS_FLOAT: u32 = 1;
pub const FORMAT_FLAG_IS_SIGNED_INTEGER: u32 = 4;
pub const FORMAT_FLAG_IS_PACKED: u32 = 8;

#[repr(C)]
pub struct AudioTimeStamp {
    pub sample_time: f64,
    pub host_time: u64,
    pub rate_scalar: f64,
    pub word_clock_time: u64,
    pub smpte_time: [u8; 24],
    pub flags: u32,
    pub reserved: u32,
}

#[repr(C)]
pub struct AudioBuffer {
    pub channels: u32,
    pub byte_size: u32,
    pub data: *mut c_void,
}

/// An `AudioBufferList` with one (interleaved) buffer.
#[repr(C)]
pub struct AudioBufferList {
    pub count: u32,
    pub buffers: [AudioBuffer; 1],
}

pub type RenderCallback = unsafe extern "C" fn(
    refcon: *mut c_void,
    action_flags: *mut u32,
    time_stamp: *const AudioTimeStamp,
    bus: u32,
    frames: u32,
    data: *mut AudioBufferList,
) -> OSStatus;

#[repr(C)]
pub struct AURenderCallbackStruct {
    pub proc_: RenderCallback,
    pub refcon: *mut c_void,
}

#[repr(C)]
pub struct AudioComponentDescription {
    pub component_type: u32,
    pub sub_type: u32,
    pub manufacturer: u32,
    pub flags: u32,
    pub flags_mask: u32,
}

pub const UNIT_TYPE_OUTPUT: u32 = fourcc(b"auou");
pub const UNIT_SUBTYPE_DEFAULT_OUTPUT: u32 = fourcc(b"def ");
pub const UNIT_SUBTYPE_HAL_OUTPUT: u32 = fourcc(b"ahal");
pub const MANUFACTURER_APPLE: u32 = fourcc(b"appl");

pub const PROPERTY_STREAM_FORMAT: u32 = 8;
pub const PROPERTY_MAXIMUM_FRAMES_PER_SLICE: u32 = 14;
pub const PROPERTY_SET_RENDER_CALLBACK: u32 = 23;
pub const PROPERTY_CURRENT_DEVICE: u32 = 2000;
pub const PROPERTY_ENABLE_IO: u32 = 2003;
pub const PROPERTY_CHANNEL_MAP: u32 = 2002;
pub const PROPERTY_SET_INPUT_CALLBACK: u32 = 2005;

pub const SCOPE_GLOBAL: u32 = 0;
pub const SCOPE_INPUT: u32 = 1;
pub const SCOPE_OUTPUT: u32 = 2;

#[repr(C)]
pub struct AudioObjectPropertyAddress {
    pub selector: u32,
    pub scope: u32,
    pub element: u32,
}

const SYSTEM_OBJECT: AudioObjectID = 1;
const OBJECT_SCOPE_GLOBAL: u32 = fourcc(b"glob");
const OBJECT_SCOPE_INPUT: u32 = fourcc(b"inpt");
const OBJECT_SCOPE_OUTPUT: u32 = fourcc(b"outp");
const DEFAULT_OUTPUT_DEVICE: u32 = fourcc(b"dOut");
const DEFAULT_INPUT_DEVICE: u32 = fourcc(b"dIn ");
const NOMINAL_SAMPLE_RATE: u32 = fourcc(b"nsrt");
const STREAM_CONFIGURATION: u32 = fourcc(b"slay");
const DEVICE_LATENCY: u32 = fourcc(b"ltnc");
const SAFETY_OFFSET: u32 = fourcc(b"saft");
const BUFFER_FRAME_SIZE: u32 = fourcc(b"fsiz");
const STREAMS: u32 = fourcc(b"stm#");
const OBJECT_NAME: u32 = fourcc(b"lnam");

aim_hostcall::dylib! {
    static CORE_AUDIO = c"/System/Library/Frameworks/CoreAudio.framework/CoreAudio" {
        fn AudioObjectGetPropertyDataSize(
            object: AudioObjectID,
            address: *const AudioObjectPropertyAddress,
            qualifier_size: u32,
            qualifier: *const c_void,
            size: *mut u32,
        ) -> OSStatus;
        fn AudioObjectGetPropertyData(
            object: AudioObjectID,
            address: *const AudioObjectPropertyAddress,
            qualifier_size: u32,
            qualifier: *const c_void,
            size: *mut u32,
            data: *mut c_void,
        ) -> OSStatus;
    }
}

aim_hostcall::dylib! {
    static AUDIO_TOOLBOX = c"/System/Library/Frameworks/AudioToolbox.framework/AudioToolbox" {
        fn AudioComponentFindNext(
            after: AudioComponent,
            desc: *const AudioComponentDescription,
        ) -> AudioComponent;
        fn AudioComponentInstanceNew(component: AudioComponent, unit: *mut AudioUnit) -> OSStatus;
        pub fn AudioComponentInstanceDispose(unit: AudioUnit) -> OSStatus;
        pub fn AudioUnitSetProperty(
            unit: AudioUnit,
            id: u32,
            scope: u32,
            element: u32,
            data: *const c_void,
            size: u32,
        ) -> OSStatus;
        pub fn AudioUnitGetProperty(
            unit: AudioUnit,
            id: u32,
            scope: u32,
            element: u32,
            data: *mut c_void,
            size: *mut u32,
        ) -> OSStatus;
        pub fn AudioUnitInitialize(unit: AudioUnit) -> OSStatus;
        pub fn AudioUnitUninitialize(unit: AudioUnit) -> OSStatus;
        pub fn AudioOutputUnitStart(unit: AudioUnit) -> OSStatus;
        pub fn AudioOutputUnitStop(unit: AudioUnit) -> OSStatus;
        pub fn AudioUnitRender(
            unit: AudioUnit,
            action_flags: *mut u32,
            time_stamp: *const AudioTimeStamp,
            bus: u32,
            frames: u32,
            data: *mut AudioBufferList,
        ) -> OSStatus;
    }
}

aim_hostcall::dylib! {
    static CORE_FOUNDATION = c"/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation" {
        fn CFStringGetCString(s: CFStringRef, buf: *mut c_char, size: isize, encoding: u32) -> bool;
        fn CFRelease(cf: *const c_void);
    }
}

const UTF8: u32 = 0x0800_0100;

/// A new instance of an Apple output unit.
pub fn new_unit(sub_type: u32) -> Option<AudioUnit> {
    let desc = AudioComponentDescription {
        component_type: UNIT_TYPE_OUTPUT,
        sub_type,
        manufacturer: MANUFACTURER_APPLE,
        flags: 0,
        flags_mask: 0,
    };
    // SAFETY: plain CoreAudio calls with local out-parameters.
    unsafe {
        let component = AudioComponentFindNext(std::ptr::null_mut(), &desc);
        if component.is_null() {
            return None;
        }
        let mut unit = std::ptr::null_mut();
        (AudioComponentInstanceNew(component, &mut unit) == 0 && !unit.is_null()).then_some(unit)
    }
}

/// Set a unit property from a value.
pub fn set_property<T>(unit: AudioUnit, id: u32, scope: u32, element: u32, value: &T) -> bool {
    // SAFETY: `value` is a live T of the size passed.
    unsafe {
        AudioUnitSetProperty(
            unit,
            id,
            scope,
            element,
            (value as *const T).cast(),
            std::mem::size_of::<T>() as u32,
        ) == 0
    }
}

/// Set a unit property from an array.
pub fn set_property_slice<T>(
    unit: AudioUnit,
    id: u32,
    scope: u32,
    element: u32,
    value: &[T],
) -> bool {
    // SAFETY: `value` is a live slice of the size passed.
    unsafe {
        AudioUnitSetProperty(
            unit,
            id,
            scope,
            element,
            value.as_ptr().cast(),
            std::mem::size_of_val(value) as u32,
        ) == 0
    }
}

fn object_property<T: Default>(object: AudioObjectID, selector: u32, scope: u32) -> Option<T> {
    let address = AudioObjectPropertyAddress {
        selector,
        scope,
        element: 0,
    };
    let mut value = T::default();
    let mut size = std::mem::size_of::<T>() as u32;
    // SAFETY: the property is read into a local T of `size` bytes.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            &address,
            0,
            std::ptr::null(),
            &mut size,
            (&mut value as *mut T).cast(),
        )
    };
    (status == 0 && size as usize == std::mem::size_of::<T>()).then_some(value)
}

fn object_property_bytes(object: AudioObjectID, selector: u32, scope: u32) -> Option<Vec<u8>> {
    let address = AudioObjectPropertyAddress {
        selector,
        scope,
        element: 0,
    };
    let mut size = 0u32;
    // SAFETY: size query, then a read into a buffer of that size (as u64s,
    // so AudioBufferList and AudioObjectID arrays are aligned).
    unsafe {
        if AudioObjectGetPropertyDataSize(object, &address, 0, std::ptr::null(), &mut size) != 0 {
            return None;
        }
        let mut words = vec![0u64; (size as usize).div_ceil(8)];
        if AudioObjectGetPropertyData(
            object,
            &address,
            0,
            std::ptr::null(),
            &mut size,
            words.as_mut_ptr().cast(),
        ) != 0
        {
            return None;
        }
        let mut bytes: Vec<u8> = words.iter().flat_map(|w| w.to_ne_bytes()).collect();
        bytes.truncate(size as usize);
        Some(bytes)
    }
}

/// The default output or input device.
pub fn default_device(input: bool) -> Option<AudioObjectID> {
    let selector = if input {
        DEFAULT_INPUT_DEVICE
    } else {
        DEFAULT_OUTPUT_DEVICE
    };
    object_property::<AudioObjectID>(SYSTEM_OBJECT, selector, OBJECT_SCOPE_GLOBAL)
        .filter(|&id| id != 0)
}

fn channels(device: AudioObjectID, scope: u32) -> u32 {
    // An AudioBufferList: u32 count, then {u32 channels, u32 size, ptr}.
    let Some(bytes) = object_property_bytes(device, STREAM_CONFIGURATION, scope) else {
        return 0;
    };
    let word = |at: usize| {
        bytes
            .get(at..at + 4)
            .map_or(0, |b| u32::from_ne_bytes(b.try_into().unwrap()))
    };
    (0..word(0) as usize).map(|i| word(8 + 16 * i)).sum()
}

fn name(device: AudioObjectID) -> [u8; 64] {
    let mut out = [0u8; 64];
    if let Some(s) = object_property::<usize>(device, OBJECT_NAME, OBJECT_SCOPE_GLOBAL) {
        let s = s as CFStringRef;
        // SAFETY: a +1 CFString from the property; the buffer keeps a NUL.
        unsafe {
            CFStringGetCString(s, out.as_mut_ptr().cast(), 63, UTF8);
            CFRelease(s);
        }
    }
    out
}

/// What macOS reports about the default output or input device.
pub fn describe(input: bool) -> Device {
    let Some(device) = default_device(input) else {
        return Device::default();
    };
    let scope = if input {
        OBJECT_SCOPE_INPUT
    } else {
        OBJECT_SCOPE_OUTPUT
    };
    let rate = object_property::<f64>(device, NOMINAL_SAMPLE_RATE, OBJECT_SCOPE_GLOBAL)
        .unwrap_or(0.0)
        .round() as u32;
    let get = |selector| object_property::<u32>(device, selector, scope).unwrap_or(0);
    // The first stream's own latency adds to the device's.
    let stream_latency = object_property_bytes(device, STREAMS, scope)
        .and_then(|b| {
            b.get(0..4)
                .map(|w| u32::from_ne_bytes(w.try_into().unwrap()))
        })
        .and_then(|stream| object_property::<u32>(stream, DEVICE_LATENCY, OBJECT_SCOPE_GLOBAL))
        .unwrap_or(0);
    let channels = channels(device, scope);
    Device {
        present: (rate != 0 && channels != 0) as u32,
        sample_rate: rate,
        channels,
        buffer_frames: object_property::<u32>(device, BUFFER_FRAME_SIZE, OBJECT_SCOPE_GLOBAL)
            .unwrap_or(0),
        latency_frames: get(DEVICE_LATENCY) + get(SAFETY_OFFSET) + stream_latency,
        reserved: 0,
        name: name(device),
    }
}
