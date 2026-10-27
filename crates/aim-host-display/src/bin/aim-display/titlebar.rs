//! TaskDescription's colors on AppKit's standard window chrome.

use crate::objc::{Id, class};

/// Main thread only. A transparent title bar lets the NSWindow background
/// show through; the task's Metal view continues to cover its content.
pub fn set(window: Id, primary: u32, background: u32) {
    let argb = if primary != 0 { primary } else { background };
    let color = if argb == 0 {
        send!(class(c"NSColor"), c"windowBackgroundColor" => Id)
    } else {
        let component = |shift: u32| ((argb >> shift) & 255u32) as f64 / 255.0;
        send!(class(c"NSColor"), c"colorWithSRGBRed:green:blue:alpha:" => Id,
            f64 = component(16), f64 = component(8), f64 = component(0), f64 = component(24))
    };
    send!(window, c"setBackgroundColor:" => (), Id = color);
    send!(window, c"setTitlebarAppearsTransparent:" => (), bool = argb != 0);
}
