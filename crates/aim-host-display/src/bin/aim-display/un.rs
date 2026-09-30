//! A window host's notifications (`docs/notifications.md`): the app's
//! Android notifications as its own Mac notifications
//! (`UNUserNotificationCenter`), and what the user does with them.
//!
//! The shim asks for provisional authorization: notifications go quietly
//! to Notification Center without a prompt, until the user chooses to
//! show them prominently there. Each notification's identifier is its
//! Android key; its actions are a category's, each a button or, for a
//! `RemoteInput`, a text field; a dismissal is reported (the categories'
//! custom dismiss action). The Dock badge counts the app's notifications
//! that are not ongoing.

use std::collections::{HashMap, HashSet};
use std::ffi::{CString, c_char, c_void};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use aim_host_display::notify::{Image, Message, Post};

use crate::objc::{GlobalBlock, Id, Sel, class, nsstring, on_main, on_main_after, release, text};
use crate::objc::{class_addMethod, objc_allocateClassPair, objc_registerClassPair, sel};

#[link(name = "UserNotifications", kind = "framework")]
unsafe extern "C" {}

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    fn CGImageSourceCreateWithData(data: *const c_void, options: *const c_void) -> *const c_void;
    fn CGImageSourceCreateImageAtIndex(
        source: *const c_void,
        index: usize,
        options: *const c_void,
    ) -> *const c_void;
    fn CGImageDestinationCreateWithURL(
        url: *const c_void,
        kind: *const c_void,
        count: usize,
        options: *const c_void,
    ) -> *const c_void;
    fn CGImageDestinationAddImage(dest: *const c_void, image: *const c_void, props: *const c_void);
    fn CGImageDestinationFinalize(dest: *const c_void) -> bool;
}

unsafe extern "C" {
    fn CFDataCreate(alloc: *const c_void, bytes: *const u8, len: isize) -> *const c_void;
    fn CFStringCreateWithCString(
        alloc: *const c_void,
        s: *const c_char,
        encoding: u32,
    ) -> *const c_void;
    fn CFURLCreateFromFileSystemRepresentation(
        alloc: *const c_void,
        path: *const u8,
        len: isize,
        dir: bool,
    ) -> *const c_void;
    fn CFRelease(obj: *const c_void);
    fn CGColorSpaceCreateDeviceRGB() -> *const c_void;
    fn CGColorSpaceRelease(cs: *const c_void);
    fn CGDataProviderCreateWithCFData(data: *const c_void) -> *const c_void;
    fn CGDataProviderRelease(p: *const c_void);
    fn CGImageCreate(
        width: usize,
        height: usize,
        bits_per_component: usize,
        bits_per_pixel: usize,
        bytes_per_row: usize,
        space: *const c_void,
        info: u32,
        provider: *const c_void,
        decode: *const f64,
        interpolate: bool,
        intent: i32,
    ) -> *const c_void;
    fn CGImageRelease(image: *const c_void);
    static _NSConcreteStackBlock: [*const c_void; 32];
}

/// `kCFStringEncodingUTF8`.
const UTF8: u32 = 0x0800_0100;
/// `kCGImageAlphaPremultipliedLast`, `kCGImageAlphaLast`.
const PREMULTIPLIED_LAST: u32 = 1;
const ALPHA_LAST: u32 = 3;
/// `UNAuthorizationOption`: badge, sound, alert, provisional.
const AUTHORIZATION: usize = 1 | 2 | 4 | 64;
/// `UNNotificationCategoryOptionCustomDismissAction`.
const CUSTOM_DISMISS: usize = 1;
/// `UNNotificationInterruptionLevel`: passive, active.
const PASSIVE: usize = 0;
const ACTIVE: usize = 1;
/// `UNNotificationPresentationOption`: sound, list, banner.
const PRESENT_SOUND: usize = 1 << 1;
const PRESENT_LIST: usize = 1 << 3;
const PRESENT_BANNER: usize = 1 << 4;
const DEFAULT_ACTION: &str = "com.apple.UNNotificationDefaultActionIdentifier";
const DISMISS_ACTION: &str = "com.apple.UNNotificationDismissActionIdentifier";
/// The category of notifications without actions.
const PLAIN: &str = "aim";

struct Shown {
    /// Keys shown without a banner.
    passive: HashSet<String>,
    /// Keys that count toward the badge.
    badged: HashSet<String>,
    /// Categories by identifier (retained).
    categories: HashMap<String, usize>,
}

static SHOWN: Mutex<Option<Shown>> = Mutex::new(None);
/// Names the image files handed to Notification Center.
static FILES: AtomicU64 = AtomicU64::new(0);

fn center() -> Id {
    send!(class(c"UNUserNotificationCenter"), c"currentNotificationCenter" => Id)
}

/// The block layout's header: its invoke function follows two ints.
#[repr(C)]
struct BlockHeader {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: *const c_void,
}

/// A block that carries one boxed closure, called once with the block's
/// object argument. Built on the stack: the API copies it (`Block_copy`
/// copies the captured pointer), and the closure is freed when called.
#[repr(C)]
struct OnceBlock {
    header: BlockHeader,
    descriptor: &'static Descriptor,
    work: *mut c_void,
}

#[repr(C)]
struct Descriptor {
    reserved: usize,
    size: usize,
}

type Work = Box<dyn FnOnce(Id) + Send>;

extern "C" fn once_invoke(block: *const OnceBlock, arg: Id) {
    // SAFETY: the heap copy of a block `once_block` made; its work is
    // taken once.
    let work = unsafe { Box::from_raw((*block).work.cast::<Work>()) };
    work(arg);
}

fn once_block(f: impl FnOnce(Id) + Send + 'static) -> OnceBlock {
    static DESCRIPTOR: Descriptor = Descriptor {
        reserved: 0,
        size: size_of::<OnceBlock>(),
    };
    let work: Box<Work> = Box::new(Box::new(f));
    OnceBlock {
        header: BlockHeader {
            isa: (&raw const _NSConcreteStackBlock).cast(),
            flags: 0,
            reserved: 0,
            invoke: once_invoke as *const c_void,
        },
        descriptor: &DESCRIPTOR,
        work: Box::into_raw(work).cast(),
    }
}

fn call_block(block: Id) {
    // SAFETY: a block whose invoke takes only the block.
    unsafe {
        let invoke: extern "C" fn(Id) = std::mem::transmute((*block.cast::<BlockHeader>()).invoke);
        invoke(block)
    }
}

fn call_block_usize(block: Id, v: usize) {
    // SAFETY: a block whose invoke takes the block and an NSUInteger.
    unsafe {
        let invoke: extern "C" fn(Id, usize) =
            std::mem::transmute((*block.cast::<BlockHeader>()).invoke);
        invoke(block, v)
    }
}

extern "C" fn authorized(_block: *const GlobalBlock, granted: bool, error: Id) {
    if !granted {
        eprintln!(
            "aim-display: notifications not authorized: {}",
            text(send!(error, c"localizedDescription" => Id))
        );
    }
}

/// Starts showing notifications: the delegate and the authorization.
/// Main thread.
pub fn start() {
    *SHOWN.lock().unwrap() = Some(Shown {
        passive: HashSet::new(),
        badged: HashSet::new(),
        categories: HashMap::new(),
    });
    let c = center();
    send!(c, c"setDelegate:" => (), Id = delegate());
    // Those of an earlier run are gone or shown again once connected.
    send!(c, c"removeAllDeliveredNotifications" => ());
    static AUTHORIZED: std::sync::OnceLock<GlobalBlock> = std::sync::OnceLock::new();
    let block = AUTHORIZED.get_or_init(|| GlobalBlock::new(authorized as *const c_void));
    send!(c, c"requestAuthorizationWithOptions:completionHandler:" => (),
        usize = AUTHORIZATION, *const GlobalBlock = block);
    set_categories();
}

fn nsarray(items: &[Id]) -> Id {
    send!(class(c"NSArray"), c"arrayWithObjects:count:" => Id,
        *const Id = items.as_ptr(), usize = items.len())
}

/// The categories: the plain one and every action set seen.
fn set_categories() {
    let mut all = vec![category(PLAIN, &[])];
    if let Some(s) = SHOWN.lock().unwrap().as_ref() {
        all.extend(s.categories.values().map(|&c| c as Id));
    }
    let set = send!(class(c"NSSet"), c"setWithArray:" => Id, Id = nsarray(&all));
    send!(center(), c"setNotificationCategories:" => (), Id = set);
}

/// An autoreleased category of `actions`.
fn category(id: &str, actions: &[aim_host_display::notify::Action]) -> Id {
    let actions: Vec<Id> = actions
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let ident = nsstring(&format!("action.{i}"));
            let title = nsstring(&a.title);
            match &a.input {
                Some(placeholder) => send!(class(c"UNTextInputNotificationAction"),
                    c"actionWithIdentifier:title:options:textInputButtonTitle:textInputPlaceholder:" => Id,
                    Id = ident, Id = title, usize = 0, Id = title, Id = nsstring(placeholder)),
                None => send!(class(c"UNNotificationAction"),
                    c"actionWithIdentifier:title:options:" => Id,
                    Id = ident, Id = title, usize = 0),
            }
        })
        .collect();
    send!(class(c"UNNotificationCategory"),
        c"categoryWithIdentifier:actions:intentIdentifiers:options:" => Id,
        Id = nsstring(id), Id = nsarray(&actions), Id = nsarray(&[]), usize = CUSTOM_DISMISS)
}

/// The category of `p`'s actions, registered if new.
fn category_of(p: &Post) -> String {
    if p.actions.is_empty() {
        return PLAIN.into();
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    p.actions.hash(&mut h);
    let id = format!("aim.{:016x}", h.finish());
    let new = {
        let mut s = SHOWN.lock().unwrap();
        let s = s.as_mut().unwrap();
        if s.categories.contains_key(&id) {
            false
        } else {
            let c = crate::objc::retain(category(&id, &p.actions));
            s.categories.insert(id.clone(), c as usize);
            true
        }
    };
    if new {
        set_categories();
    }
    id
}

/// `image` as a PNG file for Notification Center to take.
fn image_file(image: &Image) -> Option<PathBuf> {
    let dir = std::env::temp_dir().join("aim-notifications");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!(
        "{}-{}.png",
        std::process::id(),
        FILES.fetch_add(1, Ordering::Relaxed)
    ));
    let bytes = path.as_os_str().as_encoded_bytes();
    // SAFETY: CoreFoundation, CoreGraphics and ImageIO objects created and
    // released here.
    unsafe {
        let image = match image {
            Image::Rgba {
                width,
                height,
                premultiplied,
                pixels,
            } => {
                let data = CFDataCreate(std::ptr::null(), pixels.as_ptr(), pixels.len() as isize);
                let provider = CGDataProviderCreateWithCFData(data);
                let space = CGColorSpaceCreateDeviceRGB();
                let info = if *premultiplied {
                    PREMULTIPLIED_LAST
                } else {
                    ALPHA_LAST
                };
                let (w, h) = (*width as usize, *height as usize);
                let image = CGImageCreate(
                    w,
                    h,
                    8,
                    32,
                    w * 4,
                    space,
                    info,
                    provider,
                    std::ptr::null(),
                    true,
                    0,
                );
                CGColorSpaceRelease(space);
                CGDataProviderRelease(provider);
                CFRelease(data);
                image
            }
            Image::Encoded(bytes) => {
                let data = CFDataCreate(std::ptr::null(), bytes.as_ptr(), bytes.len() as isize);
                let source = CGImageSourceCreateWithData(data, std::ptr::null());
                CFRelease(data);
                if source.is_null() {
                    return None;
                }
                let image = CGImageSourceCreateImageAtIndex(source, 0, std::ptr::null());
                CFRelease(source);
                image
            }
        };
        if image.is_null() {
            return None;
        }
        let url = CFURLCreateFromFileSystemRepresentation(
            std::ptr::null(),
            bytes.as_ptr(),
            bytes.len() as isize,
            false,
        );
        let png = CString::new("public.png").unwrap();
        let kind = CFStringCreateWithCString(std::ptr::null(), png.as_ptr(), UTF8);
        let dest = CGImageDestinationCreateWithURL(url, kind, 1, std::ptr::null());
        let ok = !dest.is_null() && {
            CGImageDestinationAddImage(dest, image, std::ptr::null());
            CGImageDestinationFinalize(dest)
        };
        if !dest.is_null() {
            CFRelease(dest);
        }
        CFRelease(kind);
        CFRelease(url);
        CGImageRelease(image);
        ok.then_some(path)
    }
}

/// Tell the server whether the Mac now shows `key`: once it is as
/// `expected`, or after a second look 2 s later (Notification Center
/// takes a moment for an attachment).
fn report(key: String, error: Option<String>, expected: bool, again: bool) {
    let block = once_block(move |delivered: Id| {
        let n = send!(delivered, c"count" => usize);
        let shown = (0..n).any(|i| {
            let note = send!(delivered, c"objectAtIndex:" => Id, usize = i);
            let request = send!(note, c"request" => Id);
            text(send!(request, c"identifier" => Id)) == key
        });
        if shown != expected && error.is_none() && again {
            on_main_after(2000, move || report(key, None, expected, false));
            return;
        }
        crate::shim::notify(&Message::Shown { key, shown, error });
    });
    send!(center(), c"getDeliveredNotificationsWithCompletionHandler:" => (),
        *const OnceBlock = &block);
}

/// Show or update `p`. Main thread.
fn post(p: &Post) {
    let content = send!(class(c"UNMutableNotificationContent"), c"new" => Id);
    // Notification Center drops a notification without text; one whose
    // content is only a custom view is titled with the app's name.
    let title = if p.title.is_empty() && p.body.is_empty() {
        crate::shim::app_name().unwrap_or_default()
    } else {
        p.title.clone()
    };
    send!(content, c"setTitle:" => (), Id = nsstring(&title));
    send!(content, c"setSubtitle:" => (), Id = nsstring(&p.subtitle));
    send!(content, c"setBody:" => (), Id = nsstring(&p.body));
    send!(content, c"setThreadIdentifier:" => (), Id = nsstring(&p.thread));
    send!(content, c"setCategoryIdentifier:" => (), Id = nsstring(&category_of(p)));
    send!(content, c"setInterruptionLevel:" => (), usize = if p.passive { PASSIVE } else { ACTIVE });
    if !p.passive {
        let sound = send!(class(c"UNNotificationSound"), c"defaultSound" => Id);
        send!(content, c"setSound:" => (), Id = sound);
    }
    if let Some(file) = p.image.as_ref().and_then(image_file) {
        let url = send!(class(c"NSURL"), c"fileURLWithPath:" => Id,
            Id = nsstring(&file.to_string_lossy()));
        let mut error: Id = std::ptr::null_mut();
        let attachment = send!(class(c"UNNotificationAttachment"),
            c"attachmentWithIdentifier:URL:options:error:" => Id,
            Id = nsstring("image"), Id = url, Id = std::ptr::null_mut(), *mut Id = &mut error);
        if attachment.is_null() {
            let _ = std::fs::remove_file(&file);
        } else {
            send!(content, c"setAttachments:" => (), Id = nsarray(&[attachment]));
        }
    }
    let request = send!(class(c"UNNotificationRequest"),
        c"requestWithIdentifier:content:trigger:" => Id,
        Id = nsstring(&p.key), Id = content, Id = std::ptr::null_mut());
    release(content);
    {
        let mut s = SHOWN.lock().unwrap();
        let s = s.as_mut().unwrap();
        if p.passive {
            s.passive.insert(p.key.clone());
        } else {
            s.passive.remove(&p.key);
        }
        if p.badge {
            s.badged.insert(p.key.clone());
        } else {
            s.badged.remove(&p.key);
        }
    }
    badge();
    let key = p.key.clone();
    let block = once_block(move |error: Id| {
        let error = (!error.is_null()).then(|| text(send!(error, c"localizedDescription" => Id)));
        on_main(move || report(key, error, true, true));
    });
    send!(center(), c"addNotificationRequest:withCompletionHandler:" => (),
        Id = request, *const OnceBlock = &block);
}

fn remove(key: &str) {
    let ids = nsarray(&[nsstring(key)]);
    let c = center();
    send!(c, c"removeDeliveredNotificationsWithIdentifiers:" => (), Id = ids);
    send!(c, c"removePendingNotificationRequestsWithIdentifiers:" => (), Id = ids);
    if let Some(s) = SHOWN.lock().unwrap().as_mut() {
        s.passive.remove(key);
        s.badged.remove(key);
    }
    badge();
    report(key.to_string(), None, false, true);
}

/// The Dock badge: the notifications that count, if any.
fn badge() {
    let n = SHOWN.lock().unwrap().as_ref().map_or(0, |s| s.badged.len());
    let app = send!(class(c"NSApplication"), c"sharedApplication" => Id);
    let tile = send!(app, c"dockTile" => Id);
    let label = if n == 0 {
        std::ptr::null_mut()
    } else {
        nsstring(&n.to_string())
    };
    send!(tile, c"setBadgeLabel:" => (), Id = label);
}

/// A message from the server.
pub fn handle(m: Message) {
    on_main(move || {
        let _pool = crate::objc::Pool::new();
        match &m {
            Message::Post(p) => post(p),
            Message::Remove { key, .. } => remove(key),
            _ => {}
        }
    });
}

/// `userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:`
extern "C" fn did_receive(_: Id, _: Sel, _center: Id, response: Id, done: Id) {
    let note = send!(response, c"notification" => Id);
    let request = send!(note, c"request" => Id);
    let key = text(send!(request, c"identifier" => Id));
    let action = text(send!(response, c"actionIdentifier" => Id));
    let m = match action.as_str() {
        DEFAULT_ACTION => Some(Message::Click { key }),
        DISMISS_ACTION => Some(Message::Dismiss { key }),
        a => a
            .strip_prefix("action.")
            .and_then(|i| i.parse().ok())
            .map(|index| {
                let typed = send!(response, c"isKindOfClass:" => bool,
                    Id = class(c"UNTextInputNotificationResponse"));
                let reply = typed.then(|| text(send!(response, c"userText" => Id)));
                Message::Action { key, index, reply }
            }),
    };
    if let Some(m) = m {
        crate::shim::notify(&m);
    }
    call_block(done);
}

/// `userNotificationCenter:willPresentNotification:withCompletionHandler:`:
/// shown while the app is in front, as when it is not.
extern "C" fn will_present(_: Id, _: Sel, _center: Id, note: Id, done: Id) {
    let request = send!(note, c"request" => Id);
    let key = text(send!(request, c"identifier" => Id));
    let passive = SHOWN
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|s| s.passive.contains(&key));
    let options = if passive {
        PRESENT_LIST
    } else {
        PRESENT_LIST | PRESENT_BANNER | PRESENT_SOUND
    };
    call_block_usize(done, options);
}

fn delegate() -> Id {
    // SAFETY: registers a new NSObject subclass with its methods, once.
    let cls = unsafe {
        let cls =
            objc_allocateClassPair(class(c"NSObject"), c"AIMNotificationDelegate".as_ptr(), 0);
        class_addMethod(
            cls,
            sel(c"userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:"),
            did_receive as *const c_void,
            c"v@:@@@?".as_ptr(),
        );
        class_addMethod(
            cls,
            sel(c"userNotificationCenter:willPresentNotification:withCompletionHandler:"),
            will_present as *const c_void,
            c"v@:@@@?".as_ptr(),
        );
        objc_registerClassPair(cls);
        cls
    };
    send!(cls, c"new" => Id)
}
