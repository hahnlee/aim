//! EGL displays that reach Metal only on first use (`docs/gles-driver.md`,
//! "Displays and fork").
//!
//! ANGLE's `eglGetPlatformDisplay` already creates the process's Metal
//! device (`MTLCreateSystemDefaultDevice`, which connects to the window
//! server through SkyLight) to ask whether the GPU is supported. Zygote
//! calls `eglGetDisplay` to preload the driver and then forks every app
//! without exec.
//!
//! So the guest never sees ANGLE's display. Getting a display records the
//! request and returns a handle of ours; the first call that passes the
//! handle (`eglInitialize`) gets ANGLE's display in the calling process.
//! A guest fork makes a fresh process (the syscall layer's `fork`), which
//! gets the handles ([`fork_state`]) but none of ANGLE's displays: each is
//! made again on its first use there, and initialized if it was in the
//! parent, since the guest's EGL will not ask again.
//!
//! In a fork child of a process that had loaded ANGLE (an app forked from
//! zygote), asking for a display starts making it on a host thread
//! ([`prefetch`]): HWUI asks for it when the app binds, to preload the
//! driver, and draws its first frame much later. The ~40 ms of Metal device
//! creation then leaves the RenderThread's path to the first frame.

use std::sync::Mutex;

use crate::resolved;

const EGL_NONE: isize = 0x3038;

/// Handles are small numbers from here on: never 0 and never an address.
const HANDLE_BASE: usize = 0x6470_0000;

/// How the guest asked for a display, replayed on first use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// `eglGetDisplay(native)`.
    Native(usize),
    /// `eglGetPlatformDisplay(platform, native, attribs)`, the attributes
    /// up to and including `EGL_NONE`.
    Platform {
        platform: u32,
        native: usize,
        attribs: Vec<isize>,
    },
}

struct Slot {
    request: Request,
    /// ANGLE's display once made in this process; 0 before.
    display: usize,
    /// A host thread was started to make it.
    prefetched: bool,
    /// The guest initialized it (here or in a parent before fork).
    initialized: bool,
}

/// The displays handed to the guest; a handle is `HANDLE_BASE` plus the
/// slot's index.
#[derive(Default)]
pub struct Displays {
    slots: Vec<Slot>,
}

impl Displays {
    /// The handle for `request`, the same for the same request.
    pub fn get(&mut self, request: Request) -> usize {
        let i = match self.slots.iter().position(|s| s.request == request) {
            Some(i) => i,
            None => {
                self.slots.push(Slot {
                    request,
                    display: 0,
                    prefetched: false,
                    initialized: false,
                });
                self.slots.len() - 1
            }
        };
        HANDLE_BASE + i
    }

    fn slot(&mut self, handle: usize) -> Option<&mut Slot> {
        self.slots.get_mut(handle.checked_sub(HANDLE_BASE)?)
    }

    /// ANGLE's display for `handle`, made by `realize` on first use (with
    /// whether the guest had initialized it); 0 when that fails. `None`
    /// when `handle` is not one of ours.
    pub fn resolve(
        &mut self,
        handle: usize,
        realize: impl FnOnce(&Request, bool) -> usize,
    ) -> Option<usize> {
        let slot = self.slot(handle)?;
        if slot.display == 0 {
            slot.display = realize(&slot.request, slot.initialized);
        }
        Some(slot.display)
    }

    /// The guest initialized the display behind `handle`.
    pub fn initialized(&mut self, handle: usize) {
        if let Some(s) = self.slot(handle) {
            s.initialized = true;
        }
    }

    /// Our handle for ANGLE's `display`, or `display` itself when it is
    /// not one we handed out.
    pub fn handle(&self, display: usize) -> usize {
        self.slots
            .iter()
            .position(|s| display != 0 && s.display == display)
            .map_or(display, |i| HANDLE_BASE + i)
    }

    /// The handles and requests, for a fork child.
    pub fn save(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut put = |v: u64| out.extend_from_slice(&v.to_le_bytes());
        put(self.slots.len() as u64);
        for s in &self.slots {
            put(s.initialized as u64);
            match &s.request {
                Request::Native(n) => {
                    put(0);
                    put(*n as u64);
                }
                Request::Platform {
                    platform,
                    native,
                    attribs,
                } => {
                    put(1);
                    put(*platform as u64);
                    put(*native as u64);
                    put(attribs.len() as u64);
                    for a in attribs {
                        put(*a as u64);
                    }
                }
            }
        }
        out
    }

    /// The displays a parent saved, none of them made here yet.
    pub fn load(b: &[u8]) -> Displays {
        let mut words = b
            .chunks_exact(8)
            .map(|c| u64::from_le_bytes(c.try_into().unwrap()));
        let mut next = || words.next().unwrap_or(0);
        let n = next();
        let mut slots = Vec::new();
        for _ in 0..n {
            let initialized = next() != 0;
            let request = if next() == 0 {
                Request::Native(next() as usize)
            } else {
                let (platform, native) = (next() as u32, next() as usize);
                let len = next().min(b.len() as u64 / 8);
                Request::Platform {
                    platform,
                    native,
                    attribs: (0..len).map(|_| next() as isize).collect(),
                }
            };
            slots.push(Slot {
                request,
                display: 0,
                prefetched: false,
                initialized,
            });
        }
        Displays { slots }
    }
}

static DISPLAYS: Mutex<Displays> = Mutex::new(Displays { slots: Vec::new() });

fn displays() -> std::sync::MutexGuard<'static, Displays> {
    DISPLAYS.lock().unwrap_or_else(|e| e.into_inner())
}

/// This process's displays, for its fork children.
pub fn fork_state() -> Vec<u8> {
    displays().save()
}

/// Take the displays of the parent this process was forked from.
pub fn restore_fork_state(b: &[u8]) {
    *displays() = Displays::load(b);
}

/// ANGLE's display for a request, in this process, initialized when
/// `initialize`.
fn realize(request: &Request, initialize: bool) -> usize {
    // SAFETY: ANGLE's entry points with their EGL signatures; the attribute
    // list ends with EGL_NONE.
    unsafe {
        let display = match request {
            Request::Native(native) => {
                let f = resolved(c"eglGetDisplay");
                if f == 0 {
                    return 0;
                }
                let f: unsafe extern "C" fn(usize) -> usize = std::mem::transmute(f);
                f(*native)
            }
            Request::Platform {
                platform,
                native,
                attribs,
            } => {
                let f = resolved(c"eglGetPlatformDisplay");
                if f == 0 {
                    return 0;
                }
                let f: unsafe extern "C" fn(u32, usize, *const isize) -> usize =
                    std::mem::transmute(f);
                f(*platform, *native, attribs.as_ptr())
            }
        };
        let init = resolved(c"eglInitialize");
        if display != 0 && initialize && init != 0 {
            let init: unsafe extern "C" fn(usize, *mut i32, *mut i32) -> u32 =
                std::mem::transmute(init);
            init(display, std::ptr::null_mut(), std::ptr::null_mut());
        }
        display
    }
}

/// Copy a guest attribute list of `EGLint` or `EGLAttrib` (`i64`) values
/// as `EGLAttrib`s, up to and including `EGL_NONE` (just `EGL_NONE` for
/// null).
///
/// # Safety
/// `list` must be null or a NONE-terminated list of attribute pairs.
unsafe fn attribs<T: Copy + Into<i64>>(list: u64) -> Vec<isize> {
    let mut out = Vec::new();
    let mut p = list as *const T;
    // SAFETY: caller contract.
    unsafe {
        while !p.is_null() {
            let k = (*p).into() as isize;
            if k == EGL_NONE {
                break;
            }
            out.extend([k, (*p.add(1)).into() as isize]);
            p = p.add(2);
        }
    }
    out.push(EGL_NONE);
    out
}

/// The handle for a display the guest asks for with forwarded entry point
/// `name` and register image `regs`, or `None` when `name` does not get a
/// display.
///
/// # Safety
/// `regs` must be that entry point's arguments.
pub unsafe fn get(name: &std::ffi::CStr, regs: &[u64; 16]) -> Option<u64> {
    // SAFETY (both): the attribute list of the call.
    let request = match name.to_bytes() {
        b"eglGetDisplay" => Request::Native(regs[0] as usize),
        b"eglGetPlatformDisplay" => Request::Platform {
            platform: regs[0] as u32,
            native: regs[1] as usize,
            attribs: unsafe { attribs::<i64>(regs[2]) },
        },
        b"eglGetPlatformDisplayEXT" => Request::Platform {
            platform: regs[0] as u32,
            native: regs[1] as usize,
            attribs: unsafe { attribs::<i32>(regs[2]) },
        },
        _ => return None,
    };
    Some(displays().get(request) as u64)
}

/// Start making the display behind `handle` on a host thread, once.
pub fn prefetch(handle: usize) {
    {
        let mut d = displays();
        let Some(slot) = d.slot(handle) else { return };
        if slot.prefetched || slot.display != 0 {
            return;
        }
        slot.prefetched = true;
    }
    let _ = std::thread::Builder::new()
        .name("gpu-display".into())
        .spawn(move || {
            let pool = crate::metal::pool_push();
            host(handle);
            crate::metal::pool_pop(pool);
        });
}

/// ANGLE's display for a display argument of the guest's: its display when
/// `display` is one of our handles, else `display` unchanged.
pub fn host(display: usize) -> usize {
    displays().resolve(display, realize).unwrap_or(display)
}

/// Our handle for ANGLE's display `display` (`eglGetCurrentDisplay`).
pub fn guest(display: usize) -> usize {
    displays().handle(display)
}

/// `eglInitialize` of the guest's `display` succeeded.
pub fn initialized(display: usize) {
    displays().initialized(display);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metal() -> Request {
        Request::Platform {
            platform: 0x3202,
            native: 0,
            attribs: vec![0x3203, 0x3489, EGL_NONE],
        }
    }

    #[test]
    fn getting_a_display_creates_nothing() {
        let mut d = Displays::default();
        let h = d.get(metal());
        assert_ne!(h, 0);
        assert_eq!(d.get(metal()), h, "one handle per request");
        assert_ne!(d.get(Request::Native(0)), h);
        assert_eq!(d.slots[0].display, 0);
    }

    #[test]
    fn the_first_use_creates_the_display_once() {
        let mut d = Displays::default();
        let h = d.get(metal());
        let mut calls = 0;
        let mut realize = |r: &Request, init: bool| {
            assert_eq!((r, init), (&metal(), false));
            calls += 1;
            0x5000
        };
        assert_eq!(d.resolve(h, &mut realize), Some(0x5000));
        assert_eq!(d.resolve(h, &mut realize), Some(0x5000));
        assert_eq!(calls, 1);
        assert_eq!(d.handle(0x5000), h);
        assert_eq!(d.handle(0x6000), 0x6000, "not ours");
        assert_eq!(d.handle(0), 0);
    }

    #[test]
    fn other_values_pass_through() {
        let mut d = Displays::default();
        d.get(metal());
        assert_eq!(d.resolve(0, |_, _| unreachable!()), None);
        assert_eq!(d.resolve(0x3000, |_, _| unreachable!()), None);
        assert_eq!(d.resolve(HANDLE_BASE + 1, |_, _| unreachable!()), None);
    }

    #[test]
    fn a_failed_creation_is_retried() {
        let mut d = Displays::default();
        let h = d.get(metal());
        assert_eq!(d.resolve(h, |_, _| 0), Some(0));
        assert_eq!(d.resolve(h, |_, _| 0x5000), Some(0x5000));
    }

    #[test]
    fn a_fork_child_makes_the_displays_again_with_the_same_handles() {
        let mut d = Displays::default();
        let native = d.get(Request::Native(0));
        let h = d.get(metal());
        assert_eq!(d.resolve(h, |_, _| 0x5000), Some(0x5000));
        d.initialized(h);
        let mut child = Displays::load(&d.save());
        assert_eq!(child.get(metal()), h);
        assert_eq!(child.get(Request::Native(0)), native);
        // Made again on first use, initialized as in the parent.
        assert_eq!(
            child.resolve(h, |_, init| if init { 0x7000 } else { 1 }),
            Some(0x7000)
        );
        assert_eq!(
            child.resolve(native, |_, init| if init { 1 } else { 0x8000 }),
            Some(0x8000)
        );
    }

    #[test]
    fn attribute_lists_are_copied_to_egl_attribs() {
        let ints = [0x3203i32, 0x3489, 0x3038, 99];
        let wide = [0x3203isize, 0x3489, 0x3038, 99];
        // SAFETY: NONE-terminated lists.
        unsafe {
            assert_eq!(
                attribs::<i32>(ints.as_ptr() as u64),
                vec![0x3203, 0x3489, EGL_NONE]
            );
            assert_eq!(
                attribs::<i64>(wide.as_ptr() as u64),
                vec![0x3203, 0x3489, EGL_NONE]
            );
            assert_eq!(attribs::<i32>(0), vec![EGL_NONE]);
        }
    }
}
