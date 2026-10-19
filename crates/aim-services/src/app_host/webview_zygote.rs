//! The WebView zygote (`WebViewZygote` at `android-16.0.0_r1`): a child
//! zygote of the primary one (`ZygoteProcess.startChildZygote` with
//! `WebViewZygoteInit`) that has preloaded the WebView provider
//! (`preloadApp`), which starts the provider's isolated services: its
//! renderers. Multiprocess WebView is on once a provider is set
//! (`onWebViewProviderChanged`); the child zygote starts when first needed
//! (`getProcess`), as the WebView update service's `ensureZygoteStarted`
//! asks for it once the provider's RELRO files are made.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::io::Read;
use std::sync::{Arc, Mutex};

use aim_binder_host::parcel::{EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::WriteParcelable;

use super::{AppHost, PER_USER_RANGE, zygote};
use crate::package::info::ApplicationInfo;

/// `Process.WEBVIEW_ZYGOTE_UID`, `FIRST_ISOLATED_UID`.
const WEBVIEW_ZYGOTE_UID: u32 = 1053;
const FIRST_ISOLATED_UID: u32 = 99_000;
const WEBVIEW_ZYGOTE_INIT: &str = "com.android.internal.os.WebViewZygoteInit";

/// The running child zygote: its socket, pid and session.
struct Child {
    pid: i32,
    session: zygote::Session,
}

/// `WebViewZygote`'s state: the provider (`sPackage`), and its zygote.
#[derive(Default)]
pub(super) struct WebViewZygote {
    package: Mutex<Option<Arc<ApplicationInfo>>>,
    child: Mutex<Option<Child>>,
}

/// A random UUID, as `UUID.randomUUID().toString()` makes one.
fn random_uuid() -> std::io::Result<String> {
    let mut b = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut b)?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: Vec<String> = b.iter().map(|x| format!("{x:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        h[..4].concat(),
        h[4..6].concat(),
        h[6..8].concat(),
        h[8..10].concat(),
        h[10..].concat()
    ))
}

impl AppHost {
    /// `WebViewZygote.onWebViewProviderChanged`: the provider whose
    /// renderers the zygote starts, a running zygote stopped.
    pub(super) fn webview_provider_changed(&self, info: Arc<ApplicationInfo>) {
        *self.webview_zygote.package.lock().unwrap() = Some(info);
        if let Some(child) = self.webview_zygote.child.lock().unwrap().take()
            && let Err(e) = self.call_agent(super::KILL_PROCESS, "killProcess", |data| {
                data.write_i32(child.pid)
            })
        {
            eprintln!(
                "app host: webview_zygote: stop {}: {}",
                child.pid, e.message
            );
        }
    }

    /// `WebViewZygote.getPackageName` when multiprocess is on
    /// (`isMultiprocessEnabled`).
    pub(super) fn webview_zygote_package(&self) -> Option<String> {
        self.webview_zygote
            .package
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|i| i.item.package_name.clone())
    }

    /// `ensureZygoteStarted` (`WebViewZygote.getProcess`): the zygote
    /// started if it does not run.
    pub(super) fn ensure_webview_zygote(&self) {
        let mut child = self.webview_zygote.child.lock().unwrap();
        if child.is_none() {
            match self.start_webview_zygote() {
                Ok(c) => *child = Some(c),
                Err(e) => eprintln!(
                    "app host: webview_zygote: Error connecting to webview zygote: {}",
                    e.message
                ),
            }
        }
    }

    /// `Process.startWebView`: `start` through the WebView zygote, started
    /// first if it does not run (`getProcess`); the new process's pid.
    pub(super) fn start_webview(&self, start: &zygote::Start) -> Result<i32, Exception> {
        let state = |e: String| Exception::new(EX_ILLEGAL_STATE, format!("webview_zygote: {e}"));
        let mut child = self.webview_zygote.child.lock().unwrap();
        if child.is_none() {
            *child = Some(self.start_webview_zygote()?);
        }
        match child.as_mut().unwrap().session.start(start) {
            Ok(pid) => Ok(pid),
            Err(e) => {
                *child = None;
                Err(state(e.to_string()))
            }
        }
    }

    /// `connectToZygoteIfNeededLocked`: the child zygote started through
    /// the primary one, waited for and the provider preloaded.
    fn start_webview_zygote(&self) -> Result<Child, Exception> {
        let state = |e: String| Exception::new(EX_ILLEGAL_STATE, format!("webview_zygote: {e}"));
        let info = self
            .webview_zygote
            .package
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| state("Cannot connect to zygote, no package specified".into()))?;
        let inputs = self.inputs()?;
        let configured = self
            .configured
            .get()
            .ok_or_else(|| state("app host not configured".into()))?;
        let package = info.item.package_name.clone().unwrap_or_default();
        let app_id = info.uid as u32 % PER_USER_RANGE;
        let socket = format!(
            "{WEBVIEW_ZYGOTE_INIT}/{}",
            random_uuid().map_err(|e| state(e.to_string()))?
        );
        let abis = (configured.properties)("ro.product.cpu.abilist").unwrap_or_default();
        let start = zygote::Start {
            uid: WEBVIEW_ZYGOTE_UID,
            // `UserHandle.getSharedAppGid`: access to the shared app GID for
            // ART profiles.
            gids: vec![50_000 + app_id - 10_000],
            runtime_flags: super::memory_safety(
                &inputs.compat,
                &package,
                info.flags,
                info.target_sdk_version,
            ),
            mount_external: zygote::MountExternal::None,
            target_sdk_version: 0,
            nice_name: "webview_zygote".into(),
            seinfo: "webview_zygote".into(),
            instruction_set: None,
            app_data_dir: None,
            start_child_zygote: true,
            package_name: None,
            is_top_app: false,
            pkg_data_info: Vec::new(),
            allowlisted_data_info: Vec::new(),
            bind_mount_app_storage_dirs: false,
            bind_mount_app_data_dirs: true,
            disabled_compat_changes: Vec::new(),
            process_class: WEBVIEW_ZYGOTE_INIT.into(),
            extra_args: vec![
                format!("--zygote-socket={socket}"),
                format!("--abi-list={abis}"),
                format!("--uid-range-start={FIRST_ISOLATED_UID}"),
                format!("--uid-range-end={}", i32::MAX),
            ],
        };
        let pid = {
            let mut primary = self.zygote.lock().unwrap();
            if primary.is_none() {
                let stream =
                    (configured.connect)().map_err(|e| state(format!("zygote relay: {e}")))?;
                *primary = Some(zygote::Session::new(stream));
            }
            match primary.as_mut().unwrap().start(&start) {
                Ok(pid) => pid,
                Err(e) => {
                    *primary = None;
                    return Err(state(format!(
                        "Starting child-zygote through Zygote failed: {e}"
                    )));
                }
            }
        };
        eprintln!("app host: Start proc {pid}:webview_zygote/{WEBVIEW_ZYGOTE_UID} for {package}");
        let stream = (configured.connect)().map_err(|e| state(format!("zygote relay: {e}")))?;
        let mut session =
            zygote::Session::child(stream, &socket).map_err(|e| state(e.to_string()))?;
        let mut parcel = Parcel::new();
        info.write_to(&mut parcel);
        let preloaded = session
            .preload_app(parcel.data())
            .map_err(|e| state(format!("preloadApp: {e}")))?;
        if preloaded != 1 {
            eprintln!("app host: webview_zygote: preloadApp of {package} answered {preloaded}");
        }
        Ok(Child { pid, session })
    }
}
