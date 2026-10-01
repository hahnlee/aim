//! System panels (`docs/layers.md`, #349): the layers no task owns (the
//! IME, toasts, system dialogs), each group of overlapping ones in a
//! borderless, non-activating panel exactly over its place on the screen,
//! above the app windows, on every space. A press in a panel is a touch at
//! the display pixel under it; the key window stays the app's.

use std::cell::RefCell;
use std::sync::{Mutex, OnceLock};

use aim_host_display::layers::{Rect, groups};
use aim_host_display::windows::{Screen, view_to_display};
use aim_hostcall::display::owner;

use crate::metal::{LayerFrame, Show, Target};
use crate::objc::{CGRect, CGSize, Id, class, on_main, release};
use crate::window::NS_BACKING_STORE_BUFFERED;

/// `NSWindowStyleMaskBorderless | NSWindowStyleMaskNonactivatingPanel`.
const NONACTIVATING: usize = 1 << 7;
/// `NSStatusWindowLevel`: above floating (picture-in-picture) windows.
const STATUS_LEVEL: isize = 25;
/// `NSWindowCollectionBehaviorCanJoinAllSpaces | Stationary |
/// IgnoresCycle`.
const BEHAVIOR: usize = 1 | 1 << 4 | 1 << 6;

struct Panel {
    window: Id,
    view: Id,
    layer: Id,
    /// The display pixels it shows.
    rect: Rect,
    scale: f64,
}

thread_local! {
    /// The panels, on the main thread.
    static PANELS: RefCell<Vec<Panel>> = const { RefCell::new(Vec::new()) };
}

/// The groups the panels show, or are about to.
static GROUPS: Mutex<Vec<Rect>> = Mutex::new(Vec::new());

/// Show `f`'s system layers: a panel per group, made or closed on the
/// main thread when the groups changed.
pub fn update(f: &LayerFrame) {
    let system: Vec<Rect> = f
        .layers
        .iter()
        .filter(|l| l.owner == owner::SYSTEM)
        .map(|l| l.frame)
        .collect();
    let g = groups(&system);
    {
        let mut shown = GROUPS.lock().unwrap();
        if *shown == g {
            return;
        }
        shown.clone_from(&g);
    }
    on_main(move || apply(g));
}

fn apply(groups: Vec<Rect>) {
    let screen = screen();
    let (all, fresh) = PANELS.with(|p| {
        let mut p = p.borrow_mut();
        p.retain(|panel| {
            let keep = groups.contains(&panel.rect);
            if !keep {
                close(panel);
            }
            keep
        });
        let mut fresh = Vec::new();
        for &r in &groups {
            if !p.iter().any(|panel| panel.rect == r) {
                let panel = open(&screen, r);
                fresh.push(Target::showing(panel.layer, Some(r), Show::System));
                p.push(panel);
            }
        }
        let all = p
            .iter()
            .map(|panel| Target::showing(panel.layer, Some(panel.rect), Show::System))
            .collect();
        (all, fresh)
    });
    if let Some(d) = crate::DISPLAY.get() {
        d.set_panels(all);
        if !fresh.is_empty() {
            d.refresh(&fresh);
        }
    }
}

fn screen() -> Screen {
    let main = send!(class(c"NSScreen"), c"mainScreen" => Id);
    let frame = send!(main, c"frame" => CGRect);
    Screen {
        width: frame.size.width,
        height: frame.size.height,
        scale: send!(main, c"backingScaleFactor" => f64),
    }
}

fn device() -> Id {
    static DEVICE: OnceLock<usize> = OnceLock::new();
    *DEVICE.get_or_init(|| crate::metal::device() as usize) as Id
}

/// A panel over display pixels `r`.
fn open(screen: &Screen, r: Rect) -> Panel {
    let f = screen.to_frame(r);
    let size = CGSize {
        width: f.width,
        height: f.height,
    };
    let content = CGRect {
        x: f.x,
        y: f.y,
        size,
    };
    let w = send!(class(c"NSPanel"), c"alloc" => Id);
    let w = send!(w, c"initWithContentRect:styleMask:backing:defer:" => Id,
        CGRect = content, usize = NONACTIVATING, usize = NS_BACKING_STORE_BUFFERED, bool = false);
    send!(w, c"setReleasedWhenClosed:" => (), bool = false);
    // The server is an accessory app, never active.
    send!(w, c"setHidesOnDeactivate:" => (), bool = false);
    send!(w, c"setLevel:" => (), isize = STATUS_LEVEL);
    send!(w, c"setCollectionBehavior:" => (), usize = BEHAVIOR);
    send!(w, c"setOpaque:" => (), bool = false);
    let clear = send!(class(c"NSColor"), c"clearColor" => Id);
    send!(w, c"setBackgroundColor:" => (), Id = clear);
    send!(w, c"setHasShadow:" => (), bool = false);
    let view = crate::input::view(CGRect {
        size,
        ..Default::default()
    });
    let (pw, ph) = ((r[2] - r[0]).max(1) as u32, (r[3] - r[1]).max(1) as u32);
    let layer = crate::window::metal_layer(device(), pw, ph, screen.scale, "topLeft");
    send!(layer, c"setOpaque:" => (), bool = false);
    send!(view, c"setLayer:" => (), Id = layer);
    send!(view, c"setWantsLayer:" => (), bool = true);
    send!(w, c"setContentView:" => (), Id = view);
    send!(w, c"orderFrontRegardless" => ());
    Panel {
        window: w,
        view,
        layer,
        rect: r,
        scale: screen.scale,
    }
}

fn close(p: &Panel) {
    send!(p.window, c"close" => ());
    release(p.layer);
    release(p.view);
    release(p.window);
}

/// Where view point (`x`, `y`) of `view` (`view_height` points tall) is,
/// when `view` is a panel's: the display pixel, the panel's display
/// pixels and the display pixels per point.
pub fn locate(view: Id, x: f64, y: f64, view_height: f64) -> Option<(f64, f64, Rect, f64)> {
    PANELS.with(|p| {
        let p = p.borrow();
        let panel = p.iter().find(|panel| panel.view == view)?;
        let (px, py) = view_to_display(
            x,
            y,
            view_height,
            [panel.rect[0], panel.rect[1]],
            panel.scale,
        );
        Some((px, py, panel.rect, panel.scale))
    })
}
