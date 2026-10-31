//! Accessibility's native window state; policy and UiAutomation stay in the original service.
use super::*;
use aim_binder_host::local::{LocalProcess, Strong};
use aim_service_aidl::dev_aim_server_iaccessibilitywindowcallback as callback;
use aim_service_aidl::dev_aim_server_iaccessibilitywindowhost as api;

#[derive(Default)]
pub(super) struct Accessibility {
    tokens: Mutex<HashMap<u32, Arc<Binding>>>,
    callback: Mutex<Option<Arc<Binding>>>,
    ui: Mutex<Option<Arc<Binding>>>,
    reports: Mutex<(bool, VecDeque<(Arc<Binding>, u32, Parcel, Vec<Strong>)>)>,
    pub(super) rotation_frozen: Mutex<bool>,
}
struct Binding {
    process: Arc<LocalProcess>,
    remote: Strong,
    death: u64,
}
impl Drop for Binding {
    fn drop(&mut self) {
        self.process.clear_death(&self.remote, self.death);
    }
}
struct Snapshot(Binder, [i32; 10]);
impl WriteParcelable for Snapshot {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(68);
        p.write_binder(Some(self.0));
        self.1.iter().for_each(|value| p.write_i32(*value));
    }
}
impl AppHost {
    fn accessibility_display(&self, display: i32) -> Result<(), Exception> {
        self.display()?;
        if display != 0 {
            return Err(Exception::illegal_argument("unknown accessibility display"));
        }
        Ok(())
    }
    fn remove_overlay_token(&self, handle: u32) {
        let state = &self.windows.accessibility;
        state.tokens.lock().unwrap().remove(&handle);
        let windows: Vec<_> = self
            .windows
            .windows
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, w)| w.attrs.token == Some(Binder::Handle(handle)))
            .map(|(h, _)| *h)
            .collect();
        for window in windows {
            let removed = self.remove_window(window);
            self.remove_layers(removed.and_then(|w| w.container).into_iter().collect());
        }
        self.update_holds_screen();
    }
    fn accessibility_binding(
        &self,
        binder: Option<Binder>,
        kind: Option<bool>,
    ) -> Result<Arc<Binding>, Exception> {
        let Some(Binder::Handle(handle)) = binder else {
            return Err(Exception::illegal_argument(
                "remote accessibility binder required",
            ));
        };
        let remote = self.process.strong(handle);
        let weak = self.me.clone();
        let death = self.process.link_to_death(
            &remote,
            Box::new(move || {
                if let Some(host) = weak.upgrade() {
                    let state = &host.windows.accessibility;
                    match kind {
                        None => host.remove_overlay_token(handle),
                        Some(ui) => {
                            let slot = if ui { &state.ui } else { &state.callback };
                            let mut slot = slot.lock().unwrap();
                            if slot.as_ref().is_some_and(|b| b.remote.handle == handle) {
                                *slot = None;
                            }
                        }
                    }
                }
            }),
        );
        Ok(Arc::new(Binding {
            process: self.process.clone(),
            remote,
            death,
        }))
    }
    fn accessibility_report(
        &self,
        remote: Arc<Binding>,
        code: u32,
        data: Parcel,
        held: Vec<Strong>,
    ) {
        let state = &self.windows.accessibility;
        let mut reports = state.reports.lock().unwrap();
        reports.1.push_back((remote, code, data, held));
        if reports.0 {
            return;
        }
        reports.0 = true;
        drop(reports);
        loop {
            let report = {
                let mut reports = state.reports.lock().unwrap();
                let report = reports.1.pop_front();
                if report.is_none() {
                    reports.0 = false;
                }
                report
            };
            let Some((remote, code, data, _held)) = report else {
                break;
            };
            if let Err(e) = remote.remote.transact(code, &data, true) {
                eprintln!("app host: accessibility window callback: {e}");
            }
        }
    }
    pub(super) fn accessibility_windows_changed(&self) -> Result<(), Exception> {
        let Some(remote) = self.windows.accessibility.callback.lock().unwrap().clone() else {
            return Ok(());
        };
        let display = self.display()?.0.clone();
        let focused = {
            let focus = self.windows.focus.lock().unwrap();
            focus.current.as_ref().map(|(id, _)| *id)
        };
        let windows = self.windows.windows.lock().unwrap();
        let mut rows: Vec<_> = windows.values().filter(|w| w.layer.is_some()).collect();
        rows.sort_by_key(|w| std::cmp::Reverse(w.id));
        let focus = windows
            .values()
            .find(|w| focused == Some(w.id))
            .map(|w| w._client.binder());
        let mut held: Vec<_> = rows
            .iter()
            .map(|w| self.process.strong(w._client.handle))
            .collect();
        if let Some(Binder::Handle(h)) = focus {
            held.push(self.process.strong(h));
        }
        let values: Vec<_> = rows
            .iter()
            .map(|w| {
                let r = w.frames.frame;
                Some(Snapshot(
                    w._client.binder(),
                    [
                        w.attrs.kind,
                        w.attrs.flags,
                        w.attrs.private_flags,
                        w.uid as i32,
                        r.left,
                        r.top,
                        r.right,
                        r.bottom,
                        i32::from(focused == Some(w.id)),
                        w.id as i32,
                    ],
                ))
            })
            .collect();
        let mut data = Parcel::new();
        callback::OnWindowsChanged {
            display_id: 0,
            width: display.width,
            height: display.height,
            focused: focus,
            windows: Some(values),
        }
        .write(&mut data);
        drop(windows);
        self.accessibility_report(remote, callback::ON_WINDOWS_CHANGED, data, held);
        Ok(())
    }
    pub(super) fn accessibility_rectangle(
        &self,
        window: Option<Binder>,
        rect: [i32; 4],
        owner: (u32, i32),
    ) -> Result<(), Exception> {
        let Some(Binder::Handle(window)) = window else {
            return Err(Exception::illegal_argument("window required"));
        };
        let windows = self.windows.windows.lock().unwrap();
        let w = windows
            .get(&window)
            .ok_or_else(|| Exception::illegal_argument("unknown window"))?;
        if (w.uid, w.pid) != owner {
            return Err(Exception::security("wrong window session"));
        }
        let [left, top, right, bottom] = rect;
        let mut data = Parcel::new();
        callback::OnRectangleRequested {
            display_id: 0,
            left: left + w.frames.frame.left,
            top: top + w.frames.frame.top,
            right: right + w.frames.frame.left,
            bottom: bottom + w.frames.frame.top,
        }
        .write(&mut data);
        drop(windows);
        if let Some(remote) = self.windows.accessibility.ui.lock().unwrap().clone() {
            self.accessibility_report(remote, callback::ON_RECTANGLE_REQUESTED, data, Vec::new());
        }
        Ok(())
    }
}
struct Host(Arc<AppHost>);
pub(in crate::app_host) fn service(host: Arc<AppHost>) -> Arc<dyn Service> {
    Arc::new(Host(host))
}
fn malformed(_: i32) -> Exception {
    Exception::illegal_argument("accessibility request")
}
impl Service for Host {
    fn descriptor(&self) -> &str {
        api::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.sender_euid != super::super::SYSTEM_UID {
            return reply(Err(Exception::security("system uid required")));
        }
        let result = (|| {
            let mut p = Parcel::new();
            let host = &self.0;
            let state = &host.windows.accessibility;
            match call.code {
                api::ADD_OVERLAY_TOKEN => {
                    let a = api::AddOverlayToken::read(&mut call.data).map_err(malformed)?;
                    host.accessibility_display(a.display_id)?;
                    let mut tokens = state.tokens.lock().unwrap();
                    let binding = host.accessibility_binding(a.token, None)?;
                    if tokens.contains_key(&binding.remote.handle) {
                        return Err(Exception::illegal_argument("overlay token already exists"));
                    }
                    tokens.insert(binding.remote.handle, binding);
                }
                api::REMOVE_OVERLAY_TOKEN => {
                    let a = api::RemoveOverlayToken::read(&mut call.data).map_err(malformed)?;
                    host.accessibility_display(a.display_id)?;
                    if let Some(Binder::Handle(handle)) = a.token {
                        host.remove_overlay_token(handle);
                    }
                }
                api::SET_CALLBACK | api::SET_UI_CALLBACK => {
                    let (binder, ui) = if call.code == api::SET_CALLBACK {
                        let a = api::SetCallback::read(&mut call.data).map_err(malformed)?;
                        host.accessibility_display(a.display_id)?;
                        (a.callback, false)
                    } else {
                        (
                            api::SetUiCallback::read(&mut call.data)
                                .map_err(malformed)?
                                .callback,
                            true,
                        )
                    };
                    let slot = if ui { &state.ui } else { &state.callback };
                    let mut slot = slot.lock().unwrap();
                    *slot = binder
                        .map(|b| host.accessibility_binding(Some(b), Some(ui)))
                        .transpose()?;
                    drop(slot);
                    if !ui {
                        host.accessibility_windows_changed()?;
                    }
                }
                api::COMPUTE_WINDOWS => {
                    let a = api::ComputeWindows::read(&mut call.data).map_err(malformed)?;
                    host.accessibility_display(a.display_id)?;
                    host.accessibility_windows_changed()?;
                }
                api::SET_ACCESSIBILITY_SERVICE_UIDS => {
                    let a = api::SetAccessibilityServiceUids::read(&mut call.data)
                        .map_err(malformed)?;
                    *host.tasks.accessibility_uids.lock().unwrap() = a.uids.unwrap_or_default();
                }
                _ => {
                    return Err(host.unimplemented_named(
                        "accessibility windows",
                        "outside lifecycle (#2529)",
                    ));
                }
            }
            p.write_no_exception();
            Ok(p)
        })();
        reply(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::{Reader, Result as ParcelResult};
    use aim_service_aidl::ReadParcelable;
    use std::time::{Duration, Instant};
    struct Row([i32; 10]);
    impl ReadParcelable for Row {
        fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
            assert_eq!(r.read_i32()?, 68);
            assert!(r.read_binder()?.is_some());
            let mut values = [0; 10];
            for value in &mut values {
                *value = r.read_i32()?;
            }
            Ok(Self(values))
        }
    }
    struct Recorder(Arc<AppHost>, std::sync::mpsc::Sender<Vec<[i32; 10]>>);
    impl Service for Recorder {
        fn descriptor(&self) -> &str {
            callback::DESCRIPTOR
        }
        fn transact(&self, call: &mut Call<'_>) -> Reply {
            assert!(call.is_oneway());
            // Reentrant access must never run under either window owner lock.
            assert!(self.0.windows.windows.try_lock().is_ok());
            assert!(self.0.windows.focus.try_lock().is_ok());
            let a = callback::OnWindowsChanged::<Row>::read(&mut call.data).unwrap();
            let rows = a
                .windows
                .unwrap()
                .into_iter()
                .map(|v| v.unwrap().0)
                .collect();
            self.1.send(rows).unwrap();
            Ok(Parcel::new())
        }
    }
    fn invoke(service: &dyn Service, code: u32, data: &Parcel, uid: u32) -> Result<(), Exception> {
        service
            .transact(&mut Call {
                code,
                flags: 0,
                sender_pid: 98201,
                sender_euid: uid,
                data: data.reader(),
            })
            .unwrap()
            .reader()
            .read_exception()
            .unwrap()
    }
    #[test]
    fn native_tokens_window_snapshots_unsubscribe_and_binder_death() {
        let f = super::super::focus_tests::Fixture::new();
        let display = super::super::super::display::Display::phone_for_test();
        let resources = super::super::super::display_service::Resources {
            name: "test".into(),
            density_dpi: 320,
            brightness: (1, 255, 128),
            brightness_dim: 0.1,
        };
        let installed = f.host.display.set(Arc::new((display, resources)));
        assert!(installed.is_ok());
        let service = Host(f.host.clone());
        let state = &f.host.windows.accessibility;
        let (send, receive) = std::sync::mpsc::channel();
        let next = || receive.recv_timeout(Duration::from_secs(2)).unwrap();
        let Binder::Local(ptr) = f
            .remote
            .add_service(Arc::new(Recorder(f.host.clone(), send)))
        else {
            unreachable!()
        };
        let mut p = Parcel::new();
        p.write_i64(ptr as i64);
        let remote = f.host.process.transact(0, 1, &p, false).unwrap();
        let callback = remote
            .retain_remote_binder(remote.reader().read_binder().unwrap().unwrap())
            .unwrap();
        let mut p = Parcel::new();
        api::AddOverlayToken {
            token: Some(f.observer.binder()),
            display_id: 0,
        }
        .write(&mut p);
        assert_eq!(
            invoke(&service, api::ADD_OVERLAY_TOKEN, &p, 10123)
                .unwrap_err()
                .code,
            aim_binder_host::parcel::EX_SECURITY
        );
        invoke(&service, api::ADD_OVERLAY_TOKEN, &p, 1000).unwrap();
        assert!(invoke(&service, api::ADD_OVERLAY_TOKEN, &p, 1000).is_err());
        assert_eq!(state.tokens.lock().unwrap().len(), 1);
        let subscribe = |display_id, callback| {
            let mut p = Parcel::new();
            api::SetCallback {
                display_id,
                callback,
            }
            .write(&mut p);
            invoke(&service, api::SET_CALLBACK, &p, 1000)
        };
        assert!(subscribe(9, Some(callback.binder())).is_err());
        subscribe(0, Some(callback.binder())).unwrap();
        assert!(next().is_empty());
        f.window(1, None, false);
        {
            let mut windows = f.host.windows.windows.lock().unwrap();
            let w = windows.get_mut(&1).unwrap();
            w.frames.frame = Rect {
                left: 10,
                top: 20,
                right: 110,
                bottom: 220,
            };
            w.layer = Some(surface::Layer::animation_test_layer(
                f.host.process.strong(f.observer.handle),
                1,
            ));
        }
        // Drain the add report, then the actual visible window snapshot and its removal.
        next();
        f.host.accessibility_windows_changed().unwrap();
        let rows = next();
        assert_eq!(&rows[0][4..8], &[10, 20, 110, 220]);
        f.host.remove_window(1);
        assert!(next().is_empty());
        let mut p = Parcel::new();
        api::RemoveOverlayToken {
            token: Some(f.observer.binder()),
            display_id: 0,
        }
        .write(&mut p);
        invoke(&service, api::REMOVE_OVERLAY_TOKEN, &p, 1000).unwrap();
        assert!(state.tokens.lock().unwrap().is_empty());
        subscribe(0, None).unwrap();
        f.host.accessibility_windows_changed().unwrap();
        assert!(receive.try_recv().is_err());
        let mut p = Parcel::new();
        api::AddOverlayToken {
            token: Some(f.observer.binder()),
            display_id: 0,
        }
        .write(&mut p);
        invoke(&service, api::ADD_OVERLAY_TOKEN, &p, 1000).unwrap();
        f.remote.shutdown().unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !state.tokens.lock().unwrap().is_empty() {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
}
