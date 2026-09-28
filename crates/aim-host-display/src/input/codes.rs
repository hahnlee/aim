//! The Linux input event codes the devices use
//! (`include/uapi/linux/input-event-codes.h`).

pub const EV_SYN: u16 = 0x00;
pub const EV_KEY: u16 = 0x01;
pub const EV_REL: u16 = 0x02;
pub const EV_ABS: u16 = 0x03;
pub const EV_MSC: u16 = 0x04;
pub const EV_SW: u16 = 0x05;
pub const EV_LED: u16 = 0x11;
pub const EV_SND: u16 = 0x12;
pub const EV_REP: u16 = 0x14;
pub const EV_FF: u16 = 0x15;
pub const EV_MAX: u16 = 0x1f;

pub const SYN_REPORT: u16 = 0;
pub const SYN_DROPPED: u16 = 3;

pub const KEY_MAX: u16 = 0x2ff;
pub const BTN_TOUCH: u16 = 0x14a;
pub const KEY_FN: u16 = 0x1d0;

pub const REL_WHEEL: u16 = 0x08;
pub const REL_WHEEL_HI_RES: u16 = 0x0b;
pub const REL_MAX: u16 = 0x0f;

pub const ABS_MAX: u16 = 0x3f;
pub const ABS_CNT: usize = ABS_MAX as usize + 1;
pub const ABS_MT_SLOT: u16 = 0x2f;
/// `ABS_MT_FIRST`: the first per-contact axis.
pub const ABS_MT_TOUCH_MAJOR: u16 = 0x30;
pub const ABS_MT_POSITION_X: u16 = 0x35;
pub const ABS_MT_POSITION_Y: u16 = 0x36;
pub const ABS_MT_TRACKING_ID: u16 = 0x39;
/// `ABS_MT_LAST`.
pub const ABS_MT_TOOL_Y: u16 = 0x3d;
/// Per-contact axes, `ABS_MT_FIRST..=ABS_MT_LAST`.
pub const MT_AXES: usize = (ABS_MT_TOOL_Y - ABS_MT_TOUCH_MAJOR) as usize + 1;

pub const MSC_MAX: u16 = 0x07;
pub const SW_MAX: u16 = 0x10;
pub const LED_MAX: u16 = 0x0f;
pub const SND_MAX: u16 = 0x07;
pub const FF_MAX: u16 = 0x7f;

pub const INPUT_PROP_DIRECT: u16 = 0x01;
pub const INPUT_PROP_MAX: u16 = 0x1f;

pub const BUS_VIRTUAL: u16 = 0x06;

/// `EV_VERSION`, the evdev protocol version.
pub const EV_VERSION: i32 = 0x010001;

/// Whether `code` is a per-contact (slot) axis.
pub fn is_mt_axis(code: u16) -> bool {
    (ABS_MT_TOUCH_MAJOR..=ABS_MT_TOOL_Y).contains(&code)
}

/// The highest code of an event type, as `EVIOCGBIT` sizes its bitmap;
/// None for the types it does not answer.
pub fn max_code(ev: u16) -> Option<u16> {
    Some(match ev {
        0 => EV_MAX,
        EV_KEY => KEY_MAX,
        EV_REL => REL_MAX,
        EV_ABS => ABS_MAX,
        EV_MSC => MSC_MAX,
        EV_LED => LED_MAX,
        EV_SND => SND_MAX,
        EV_FF => FF_MAX,
        EV_SW => SW_MAX,
        _ => return None,
    })
}

/// Bytes the kernel copies for a bitmap up to `max`:
/// `BITS_TO_LONGS(max) * sizeof(long)` on a 64-bit kernel.
pub fn bitmap_bytes(max: u16) -> usize {
    (max as usize).div_ceil(64) * 8
}
