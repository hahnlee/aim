//! A window host's menu bar items (`docs/notifications.md`, "The menu
//! bar"): each of its app's live notifications (an Android 16 Live
//! Update, an ongoing activity whose time runs) is a menu bar item of the
//! app's own (`NSStatusItem`), as SystemUI shows one as a status bar chip.
//! The item shows the small icon as a template image and a short text: the
//! app's short critical text, else its chronometer counting, else its
//! progress. Clicked, it shows a popover with the notification's text,
//! chronometer, progress and actions; its title opens the app as a click
//! on the notification does. The item goes when the notification does or
//! stops being live.
//!
//! An app's indicators (its microphone, camera or location in use) are
//! items too, each a symbol; clicked, they bring the app to the front. Main thread only.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;

use aim_host_display::notify::{Image, Indicator, Live, Message, Post};

use crate::objc::{CGRect, CGSize, Id, Sel, class, nsstring, on_main_after, release, retain, text};
use crate::objc::{class_addMethod, objc_allocateClassPair, objc_registerClassPair, sel};

/// `NSVariableStatusItemLength`.
const VARIABLE_LENGTH: f64 = -1.0;
/// `NSCellImagePosition`: `NSImageLeft`.
const IMAGE_LEFT: usize = 2;
/// `NSPopoverBehaviorTransient`.
const TRANSIENT: isize = 1;
/// `NSRectEdgeMinY`: below the item.
const BELOW: usize = 1;
/// `NSUserInterfaceLayoutOrientation`.
const HORIZONTAL: isize = 0;
const VERTICAL: isize = 1;
/// `NSLayoutAttributeLeading`.
const LEADING: isize = 5;
/// `NSFontWeightMedium`.
const MEDIUM: f64 = 0.23;
/// The popover's content width and margin, in points.
const WIDTH: f64 = 280.0;
const MARGIN: f64 = 12.0;
/// The item's icon side, in points.
const ICON: f64 = 16.0;
/// Button tags: the item itself (shows the popover), the popover's title
/// (opens the app); an action's is its index.
const TOGGLE: isize = -2;
const OPEN: isize = -1;
/// The tag of an indicator's item: it brings the app to the front.
const LAUNCH: isize = -3;

struct Item {
    /// The `NSStatusItem`, retained.
    item: Id,
    /// The `NSPopover`, retained.
    popover: Id,
    /// The popover's chronometer label, retained while it has one.
    clock: Id,
    live: Live,
    post: Post,
}

thread_local! {
    static ITEMS: RefCell<HashMap<String, Item>> = RefCell::new(HashMap::new());
    static TARGET: Id = target();
    /// The indicators' items (retained), by package and indicator.
    static INDICATORS: RefCell<HashMap<(String, Indicator), usize>> = RefCell::new(HashMap::new());
    /// The tick due: an older one does nothing.
    static TICK: Cell<u64> = const { Cell::new(0) };
}

#[repr(C)]
struct EdgeInsets {
    top: f64,
    left: f64,
    bottom: f64,
    right: f64,
}

fn now_ms() -> i64 {
    aim_hostcall::clock::boottime_ns() / 1_000_000
}

/// Show, update or remove `p`'s item as it is live or not.
pub fn post(p: &Post) {
    let Some(live) = p.live.as_deref().cloned() else {
        remove(&p.key);
        return;
    };
    let fresh = ITEMS.with_borrow(|items| !items.contains_key(&p.key));
    if fresh {
        let bar = send!(class(c"NSStatusBar"), c"systemStatusBar" => Id);
        let item = retain(send!(bar, c"statusItemWithLength:" => Id, f64 = VARIABLE_LENGTH));
        let button = send!(item, c"button" => Id);
        send!(button, c"setTarget:" => (), Id = TARGET.with(|t| *t));
        send!(button, c"setAction:" => (), Sel = sel(c"pressed:"));
        send!(button, c"setTag:" => (), isize = TOGGLE);
        send!(button, c"setIdentifier:" => (), Id = nsstring(&p.key));
        send!(button, c"setImagePosition:" => (), usize = IMAGE_LEFT);
        let menu_font = send!(class(c"NSFont"), c"menuBarFontOfSize:" => Id, f64 = 0.0);
        let size = send!(menu_font, c"pointSize" => f64);
        let font = send!(class(c"NSFont"), c"monospacedDigitSystemFontOfSize:weight:" => Id,
            f64 = size, f64 = 0.0);
        send!(button, c"setFont:" => (), Id = font);
        let popover = send!(class(c"NSPopover"), c"new" => Id);
        send!(popover, c"setBehavior:" => (), isize = TRANSIENT);
        ITEMS.with_borrow_mut(|items| {
            items.insert(
                p.key.clone(),
                Item {
                    item,
                    popover,
                    clock: std::ptr::null_mut(),
                    live: live.clone(),
                    post: p.clone(),
                },
            )
        });
    }
    let (item, popover, icon_changed) = ITEMS.with_borrow_mut(|items| {
        let i = items.get_mut(&p.key).unwrap();
        let changed = fresh || i.live.icon != live.icon;
        i.live = live.clone();
        i.post = p.clone();
        (i.item, i.popover, changed)
    });
    if icon_changed {
        let button = send!(item, c"button" => Id);
        send!(button, c"setImage:" => (), Id = icon(live.icon.as_ref()));
    }
    if send!(popover, c"isShown" => bool) {
        fill(&p.key);
    }
    tick();
}

/// Remove `key`'s item, if it has one.
pub fn remove(key: &str) {
    let Some(i) = ITEMS.with_borrow_mut(|items| items.remove(key)) else {
        return;
    };
    send!(i.popover, c"close" => ());
    release(i.popover);
    release(i.clock);
    let bar = send!(class(c"NSStatusBar"), c"systemStatusBar" => Id);
    send!(bar, c"removeStatusItem:" => (), Id = i.item);
    release(i.item);
}

/// Show or remove `package`'s item for `indicator`.
pub fn indicator(package: &str, indicator: Indicator, on: bool) {
    let key = (package.to_string(), indicator);
    if !on {
        if let Some(item) = INDICATORS.with_borrow_mut(|items| items.remove(&key)) {
            remove_item(item as Id);
        }
        return;
    }
    if INDICATORS.with_borrow(|items| items.contains_key(&key)) {
        return;
    }
    let (symbol, description) = match indicator {
        Indicator::Microphone => ("mic.fill", "Microphone"),
        Indicator::Camera => ("video.fill", "Camera"),
        Indicator::Location => ("location.fill", "Location"),
    };
    let item = launch_item();
    let image = send!(class(c"NSImage"), c"imageWithSystemSymbolName:accessibilityDescription:" => Id,
        Id = nsstring(symbol), Id = nsstring(description));
    let button = send!(item, c"button" => Id);
    send!(button, c"setImage:" => (), Id = image);
    INDICATORS.with_borrow_mut(|items| items.insert(key, item as usize));
}

/// A new menu bar item (retained) that brings the app to the front when
/// clicked, its image left of its text.
pub fn launch_item() -> Id {
    let bar = send!(class(c"NSStatusBar"), c"systemStatusBar" => Id);
    let item = retain(send!(bar, c"statusItemWithLength:" => Id, f64 = VARIABLE_LENGTH));
    let button = send!(item, c"button" => Id);
    send!(button, c"setTarget:" => (), Id = TARGET.with(|t| *t));
    send!(button, c"setAction:" => (), Sel = sel(c"pressed:"));
    send!(button, c"setTag:" => (), isize = LAUNCH);
    send!(button, c"setImagePosition:" => (), usize = IMAGE_LEFT);
    item
}

/// Remove a menu bar item and release it.
pub fn remove_item(item: Id) {
    let bar = send!(class(c"NSStatusBar"), c"systemStatusBar" => Id);
    send!(bar, c"removeStatusItem:" => (), Id = item);
    release(item);
}

/// An item's image: `image` as a template, else the app's icon.
pub fn icon(image: Option<&Image>) -> Id {
    let image = image
        .map(crate::un::cg_image)
        .filter(|i| !i.is_null())
        .map(|cg| {
            let size = CGSize {
                width: ICON,
                height: ICON,
            };
            let image = send!(class(c"NSImage"), c"alloc" => Id);
            let image = send!(image, c"initWithCGImage:size:" => Id,
                *const c_void = cg, CGSize = size);
            // SAFETY: the CGImage `cg_image` returned, now held by the NSImage.
            unsafe { CGImageRelease(cg) };
            send!(image, c"setTemplate:" => (), bool = true);
            send!(image, c"autorelease" => Id)
        });
    image.unwrap_or_else(|| {
        let app = send!(class(c"NSApplication"), c"sharedApplication" => Id);
        let image = send!(app, c"applicationIconImage" => Id);
        let image = send!(image, c"copy" => Id);
        send!(image, c"setSize:" => (), CGSize = CGSize { width: ICON, height: ICON });
        send!(image, c"autorelease" => Id)
    })
}

unsafe extern "C" {
    fn CGImageRelease(image: *const c_void);
}

/// The item's text at `now`.
fn title(live: &Live, now: i64) -> String {
    if !live.text.is_empty() {
        return live.text.clone();
    }
    if let Some(c) = live.clock {
        return c.text(now);
    }
    match live.progress {
        Some(p) if !p.indeterminate && p.max > 0 => {
            format!("{}%", u64::from(p.value) * 100 / u64::from(p.max))
        }
        _ => String::new(),
    }
}

/// Bring every item's text up to date, and come back when a chronometer's
/// next changes.
fn tick() {
    let now = now_ms();
    let next = ITEMS.with_borrow(|items| {
        items
            .values()
            .filter_map(|i| {
                let button = send!(i.item, c"button" => Id);
                send!(button, c"setTitle:" => (), Id = nsstring(&title(&i.live, now)));
                let clock = i.live.clock?;
                if !i.clock.is_null() {
                    send!(i.clock, c"setStringValue:" => (), Id = nsstring(&clock.text(now)));
                }
                clock.next_change_ms(now)
            })
            .min()
    });
    let due = TICK.get() + 1;
    TICK.set(due);
    if let Some(ms) = next {
        on_main_after(ms as u64, move || {
            let _pool = crate::objc::Pool::new();
            if TICK.get() == due {
                tick();
            }
        });
    }
}

/// An autoreleased label.
fn label(s: &str, font: Id, wrap: bool) -> Id {
    let make = if wrap {
        c"wrappingLabelWithString:"
    } else {
        c"labelWithString:"
    };
    let l = send!(class(c"NSTextField"), make => Id, Id = nsstring(s));
    send!(l, c"setFont:" => (), Id = font);
    if wrap {
        send!(l, c"setPreferredMaxLayoutWidth:" => (), f64 = WIDTH);
    }
    l
}

fn system_font(size: f64, bold: bool) -> Id {
    if bold {
        send!(class(c"NSFont"), c"boldSystemFontOfSize:" => Id, f64 = size)
    } else {
        send!(class(c"NSFont"), c"systemFontOfSize:" => Id, f64 = size)
    }
}

/// An autoreleased button sending `pressed:` for `key` with `tag`.
fn button(title: &str, key: &str, tag: isize) -> Id {
    let b = send!(class(c"NSButton"), c"buttonWithTitle:target:action:" => Id,
        Id = nsstring(title), Id = TARGET.with(|t| *t), Sel = sel(c"pressed:"));
    send!(b, c"setIdentifier:" => (), Id = nsstring(key));
    send!(b, c"setTag:" => (), isize = tag);
    b
}

fn stack(views: &[Id], orientation: isize) -> Id {
    let array = send!(class(c"NSArray"), c"arrayWithObjects:count:" => Id,
        *const Id = views.as_ptr(), usize = views.len());
    let s = send!(class(c"NSStackView"), c"stackViewWithViews:" => Id, Id = array);
    send!(s, c"setOrientation:" => (), isize = orientation);
    send!(s, c"setSpacing:" => (), f64 = 8.0);
    if orientation == VERTICAL {
        send!(s, c"setAlignment:" => (), isize = LEADING);
    }
    s
}

/// Build `key`'s popover content from its notification.
fn fill(key: &str) {
    let Some((popover, live, post, old_clock)) = ITEMS.with_borrow_mut(|items| {
        let i = items.get_mut(key)?;
        let old = std::mem::replace(&mut i.clock, std::ptr::null_mut());
        Some((i.popover, i.live.clone(), i.post.clone(), old))
    }) else {
        return;
    };
    release(old_clock);
    let mut views = Vec::new();
    let app = crate::shim::app_name().unwrap_or_default();
    let heading = if post.title.is_empty() {
        &app
    } else {
        &post.title
    };
    let open = button(heading, key, OPEN);
    send!(open, c"setBordered:" => (), bool = false);
    send!(open, c"setFont:" => (), Id = system_font(13.0, true));
    if heading != &app {
        views.push(label(&app, system_font(11.0, false), false));
    }
    views.push(open);
    for text in [&post.subtitle, &post.body] {
        if !text.is_empty() {
            views.push(label(text, system_font(13.0, false), true));
        }
    }
    let mut clock: Id = std::ptr::null_mut();
    if let Some(c) = live.clock {
        let font = send!(class(c"NSFont"), c"monospacedDigitSystemFontOfSize:weight:" => Id,
            f64 = 28.0, f64 = MEDIUM);
        clock = retain(label(&c.text(now_ms()), font, false));
        views.push(clock);
    }
    if let Some(p) = live.progress {
        let bar = send!(class(c"NSProgressIndicator"), c"new" => Id);
        let bar = send!(bar, c"autorelease" => Id);
        send!(bar, c"setIndeterminate:" => (), bool = p.indeterminate);
        send!(bar, c"setMinValue:" => (), f64 = 0.0);
        send!(bar, c"setMaxValue:" => (), f64 = f64::from(p.max));
        send!(bar, c"setDoubleValue:" => (), f64 = f64::from(p.value));
        if p.indeterminate {
            send!(bar, c"startAnimation:" => (), Id = std::ptr::null_mut());
        }
        let width = send!(bar, c"widthAnchor" => Id);
        let c = send!(width, c"constraintEqualToConstant:" => Id, f64 = WIDTH);
        send!(c, c"setActive:" => (), bool = true);
        views.push(bar);
    }
    // Actions that take text stay in Notification Center, which has a
    // field for them.
    let actions: Vec<Id> = post
        .actions
        .iter()
        .enumerate()
        .filter(|(_, a)| a.input.is_none())
        .map(|(i, a)| button(&a.title, key, i as isize))
        .collect();
    if !actions.is_empty() {
        views.push(stack(&actions, HORIZONTAL));
    }
    let content = stack(&views, VERTICAL);
    send!(content, c"setEdgeInsets:" => (), EdgeInsets = EdgeInsets {
        top: MARGIN,
        left: MARGIN,
        bottom: MARGIN,
        right: MARGIN,
    });
    let controller = send!(class(c"NSViewController"), c"new" => Id);
    send!(controller, c"setView:" => (), Id = content);
    send!(popover, c"setContentViewController:" => (), Id = controller);
    release(controller);
    let size = send!(content, c"fittingSize" => CGSize);
    send!(popover, c"setContentSize:" => (), CGSize = size);
    ITEMS.with_borrow_mut(|items| match items.get_mut(key) {
        Some(i) => i.clock = clock,
        None => release(clock),
    });
}

/// Show or close `key`'s popover.
fn toggle(key: &str) {
    let Some((item, popover)) =
        ITEMS.with_borrow(|items| items.get(key).map(|i| (i.item, i.popover)))
    else {
        return;
    };
    if send!(popover, c"isShown" => bool) {
        send!(popover, c"close" => ());
        return;
    }
    fill(key);
    let button = send!(item, c"button" => Id);
    let bounds = send!(button, c"bounds" => CGRect);
    send!(popover, c"showRelativeToRect:ofView:preferredEdge:" => (),
        CGRect = bounds, Id = button, usize = BELOW);
}

/// `pressed:` of the item and the popover's buttons.
extern "C" fn pressed(_: Id, _: Sel, sender: Id) {
    let key = text(send!(sender, c"identifier" => Id));
    let tag = send!(sender, c"tag" => isize);
    if tag == TOGGLE {
        toggle(&key);
        return;
    }
    if tag == LAUNCH {
        crate::shim::launch();
        return;
    }
    if let Some(popover) = ITEMS.with_borrow(|items| items.get(&key).map(|i| i.popover)) {
        send!(popover, c"close" => ());
    }
    crate::shim::notify(&match tag {
        OPEN => Message::Click { key },
        index => Message::Action {
            key,
            index: index as u32,
            reply: None,
        },
    });
}

fn target() -> Id {
    // SAFETY: registers a new NSObject subclass with its method, once.
    let cls = unsafe {
        let cls = objc_allocateClassPair(class(c"NSObject"), c"AIMStatusTarget".as_ptr(), 0);
        class_addMethod(
            cls,
            sel(c"pressed:"),
            pressed as *const c_void,
            c"v@:@".as_ptr(),
        );
        objc_registerClassPair(cls);
        cls
    };
    send!(cls, c"new" => Id)
}
