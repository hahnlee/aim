//! The original DnsResolver (the `com.android.resolv` module's
//! `libnetd_resolv.so`), which netd hosts: it registers `dnsresolver` and
//! serves `dnsproxyd`, and asks netd for the network context of each query
//! through these callbacks (`netd_resolv/resolv.h`).

use std::ffi::{CStr, c_char, c_int, c_void};

use crate::networks;

/// `struct android_net_context`.
#[repr(C)]
pub struct NetContext {
    app_netid: u32,
    app_mark: u32,
    dns_netid: u32,
    dns_mark: u32,
    uid: u32,
    flags: u32,
    pid: i32,
}

/// `struct ResolverNetdCallbacks`.
#[repr(C)]
struct Callbacks {
    check_calling_permission: extern "C" fn(*const c_char) -> bool,
    get_network_context: extern "C" fn(u32, u32, *mut NetContext),
    log: extern "C" fn(*const c_char),
    tag_socket: extern "C" fn(c_int, u32, u32, i32) -> c_int,
    /// None: every lookup proceeds.
    evaluate_domain_name: Option<extern "C" fn(*const NetContext, *const c_char) -> bool>,
}

/// Permission checks of DnsResolver's binder calls: the platform's own
/// callers (below the first app uid) only.
extern "C" fn check_calling_permission(_: *const c_char) -> bool {
    binder::ThreadState::get_calling_uid() < 10_000
}

/// NetworkController::getNetworkContext: an unset network is the caller's
/// default; the mark carries the network id.
extern "C" fn get_network_context(netid: u32, uid: u32, out: *mut NetContext) {
    let net = if netid == 0 {
        networks::default_network()
    } else {
        netid
    };
    // SAFETY: the resolver passes a valid context to fill.
    unsafe {
        out.write(NetContext {
            app_netid: net,
            app_mark: net,
            dns_netid: net,
            dns_mark: net,
            uid,
            flags: 0,
            pid: -1,
        })
    };
}

extern "C" fn log(msg: *const c_char) {
    // SAFETY: the resolver passes a NUL-terminated message.
    log::info!("{}", unsafe { CStr::from_ptr(msg) }.to_string_lossy());
}

/// Traffic tagging for per-uid statistics: there is no eBPF accounting.
extern "C" fn tag_socket(_: c_int, _: u32, _: u32, _: i32) -> c_int {
    0
}

static CALLBACKS: Callbacks = Callbacks {
    check_calling_permission,
    get_network_context,
    log,
    tag_socket,
    evaluate_domain_name: None,
};

/// Load DnsResolver from its module and start it, as netd's
/// `initDnsResolver` does.
pub fn start() -> Result<(), String> {
    // SAFETY: loading the module library and looking up its C entry point.
    unsafe {
        let lib = libc::dlopen(c"libnetd_resolv.so".as_ptr(), libc::RTLD_NOW);
        if lib.is_null() {
            return Err(CStr::from_ptr(libc::dlerror())
                .to_string_lossy()
                .into_owned());
        }
        let init = libc::dlsym(lib, c"resolv_init".as_ptr());
        if init.is_null() {
            return Err("libnetd_resolv.so has no resolv_init".into());
        }
        let init: extern "C" fn(*const c_void) -> bool = std::mem::transmute(init);
        if !init((&raw const CALLBACKS).cast()) {
            return Err("resolv_init failed".into());
        }
    }
    Ok(())
}
