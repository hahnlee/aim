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
        p.write_i32(72); // size + Binder/stability (28) + ten ints
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
                api::ON_USER_STOPPED => {
                    let a = api::OnUserStopped::read(&mut call.data).map_err(malformed)?;
                    host.tasks
                        .app_switches
                        .lock()
                        .unwrap()
                        .allowances
                        .remove(&a.user_id);
                }
                api::SET_INPUT_FILTER => {
                    let a = api::SetInputFilter::read(&mut call.data).map_err(malformed)?;
                    host.call_agent(20, "setInputFilter", |p| p.write_binder(a.filter))?;
                }
                api::SEND_INPUT_EVENT => {
                    let a = api::SendInputEvent::read(&mut call.data).map_err(malformed)?;
                    let event = a
                        .event
                        .ok_or_else(|| Exception::illegal_argument("event"))?;
                    host.call_agent(21, "sendInputEvent", |p| {
                        aim_service_aidl::write_byte_array(p, Some(&event));
                        p.write_i32(a.policy_flags | 0x04000000);
                    })?;
                }
                api::SET_ALLOW_APP_SWITCHES => {
                    let a = api::SetAllowAppSwitches::read(&mut call.data).map_err(malformed)?;
                    // UserController.isUserRunning(FLAG_OR_STOPPED): only user 0 runs here.
                    host.tasks.app_switches.lock().unwrap().set(
                        a.r#type
                            .ok_or_else(|| Exception::illegal_argument("type"))?,
                        a.uid,
                        a.user_id,
                        a.user_id == super::super::user_service::SYSTEM_USER,
                    );
                }
                api::GET_WINDOW_TRANSFORMATION_MATRIX => {
                    let a = api::GetWindowTransformationMatrix::read(&mut call.data)
                        .map_err(malformed)?;
                    let windows = host.windows.windows.lock().unwrap();
                    let window = match a.token {
                        Some(Binder::Handle(h)) => windows.get(&h),
                        _ => None,
                    };
                    // AccessibilityController / WindowState.getTransformationMatrix,
                    // android-16.0.0_r1: missing windows have an identity transform.
                    // Native windows have unit compatibility scale; place_window's
                    // surface-inset subtraction cancels when mapping back to the frame.
                    let (x, y) = window.map_or((0., 0.), |w| {
                        (w.frames.frame.left as f32, w.frames.frame.top as f32)
                    });
                    api::write_get_window_transformation_matrix_reply(
                        &mut p,
                        &Some(vec![1., 0., x, 0., 1., y, 0., 0., 1.]),
                    );
                    return Ok(p);
                }
                api::GET_WINDOW_OWNER_USER_ID | api::GET_DISPLAY_ID_FOR_WINDOW => {
                    let token = if call.code == api::GET_WINDOW_OWNER_USER_ID {
                        api::GetWindowOwnerUserId::read(&mut call.data)
                            .map_err(malformed)?
                            .token
                    } else {
                        api::GetDisplayIdForWindow::read(&mut call.data)
                            .map_err(malformed)?
                            .token
                    };
                    let windows = host.windows.windows.lock().unwrap();
                    let window = match token {
                        Some(Binder::Handle(h)) => windows.get(&h),
                        _ => None,
                    };
                    // WMS.LocalService: USER_NULL / INVALID_DISPLAY for absent windows.
                    if call.code == api::GET_WINDOW_OWNER_USER_ID {
                        api::write_get_window_owner_user_id_reply(
                            &mut p,
                            window
                                .map_or(-10000, |w| (w.uid / super::super::PER_USER_RANGE) as i32),
                        );
                    } else {
                        api::write_get_display_id_for_window_reply(
                            &mut p,
                            window.map_or(-1, |_| 0),
                        );
                    }
                    return Ok(p);
                }
                api::GET_FOCUSED_WINDOW_TOKEN_FROM_WINDOW_STATES => {
                    api::GetFocusedWindowTokenFromWindowStates::read(&mut call.data)
                        .map_err(malformed)?;
                    let focused = host
                        .windows
                        .focus
                        .lock()
                        .unwrap()
                        .current
                        .as_ref()
                        .map(|(id, _)| *id);
                    let windows = host.windows.windows.lock().unwrap();
                    let token = windows
                        .values()
                        .find(|w| Some(w.id) == focused)
                        .map(|w| w._client.binder());
                    api::write_get_focused_window_token_from_window_states_reply(&mut p, token);
                    return Ok(p);
                }
                api::SET_ACCESSIBILITY_ID_TO_SURFACE_METADATA => {
                    let a = api::SetAccessibilityIdToSurfaceMetadata::read(&mut call.data)
                        .map_err(malformed)?;
                    let mut windows = host.windows.windows.lock().unwrap();
                    if let Some(w) = a.token.and_then(|t| match t {
                        Binder::Handle(h) => windows.get_mut(&h),
                        _ => None,
                    }) {
                        if let Some(layer) = &w.container {
                            host.composer()?
                                .apply(&[(
                                    layer,
                                    surface::Change {
                                        metadata: Some((
                                            surface::METADATA_ACCESSIBILITY_ID,
                                            a.accessibility_window_id,
                                        )),
                                        ..Default::default()
                                    },
                                )])
                                .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
                        }
                        // ViewRootImpl registers before the first relayout creates the surface.
                        w.accessibility_id = a.accessibility_window_id;
                    }
                }
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
            let start = r.position();
            let size = r.read_i32()? as usize;
            assert!(r.read_binder()?.is_some());
            let mut values = [0; 10];
            for value in &mut values {
                *value = r.read_i32()?;
            }
            // Generated AccessibilityWindowData.readFromParcel advances to the
            // declared end; a short size leaves bytes for enforceNoDataAvail.
            assert_eq!(r.position() - start, size);
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
            // The fixture waits for delivery before its next mutation, so these
            // checks cannot observe an unrelated writer's lock (#2554).
            assert!(self.0.windows.windows.try_lock().is_ok());
            assert!(self.0.windows.focus.try_lock().is_ok());
            let mut p = Parcel::new();
            api::GetFocusedWindowTokenFromWindowStates {}.write(&mut p);
            invoke(
                &Host(self.0.clone()),
                api::GET_FOCUSED_WINDOW_TOKEN_FROM_WINDOW_STATES,
                &p,
                1000,
            )
            .unwrap();
            let a = callback::OnWindowsChanged::<Row>::read(&mut call.data).unwrap();
            assert_eq!(call.data.remaining(), 0);
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
    fn system_bridge_stores_allowances_without_launch_effect_and_rejects_other_uids() {
        use aim_binder_host::{local::Call, parcel::Parcel};
        use aim_service_aidl::dev_aim_server_iaccessibilitywindowhost as api;
        let f = super::super::focus_tests::Fixture::new();
        let service = super::service(f.host.clone());
        let set = |uid, user_id, sender_euid| {
            let mut p = Parcel::new();
            api::SetAllowAppSwitches {
                r#type: Some("service".into()),
                uid,
                user_id,
            }
            .write(&mut p);
            service
                .transact(&mut Call {
                    code: api::SET_ALLOW_APP_SWITCHES,
                    flags: 0,
                    sender_pid: 98201,
                    sender_euid,
                    data: p.reader(),
                })
                .unwrap()
                .reader()
                .read_exception()
                .unwrap()
        };
        assert!(set(10123, 0, 10123).is_err());
        set(10123, 0, 1000).unwrap();
        assert_eq!(
            f.host
                .background_activity_start_allowed(10123)
                .unwrap_err()
                .message,
            "no permissionmgr"
        );
        set(110123, 1, 1000).unwrap(); // Non-running users are ignored.
        let allows = |uid| {
            f.host
                .tasks
                .app_switches
                .lock()
                .unwrap()
                .allows(uid, false, false, Instant::now())
        };
        assert!(!allows(110123));
        assert!(
            !f.host
                .tasks
                .app_switches
                .lock()
                .unwrap()
                .allowances
                .contains_key(&1)
        );
        set(-1, 0, 1000).unwrap();
        assert!(!allows(10123));
        set(10123, 0, 1000).unwrap();
        let mut p = Parcel::new();
        api::OnUserStopped { user_id: 0 }.write(&mut p);
        invoke(service.as_ref(), api::ON_USER_STOPPED, &p, 1000).unwrap();
        assert!(
            !f.host
                .tasks
                .app_switches
                .lock()
                .unwrap()
                .allowances
                .contains_key(&0)
        );
        // Both public Binder routes share ATMS's switch-state owner.
        let am = super::super::super::Activity(f.host.clone());
        let atm =
            super::super::super::activity::ActivityTaskManager(f.host.clone(), Binder::Local(0));
        use aim_service_aidl::{
            android_app_iactivitymanager as am_api, android_app_iactivitytaskmanager as atm_api,
        };
        for (service, descriptor, stop, resume) in [
            (
                &am as &dyn Service,
                am_api::DESCRIPTOR,
                am_api::STOP_APP_SWITCHES,
                am_api::RESUME_APP_SWITCHES,
            ),
            (
                &atm as &dyn Service,
                atm_api::DESCRIPTOR,
                atm_api::STOP_APP_SWITCHES,
                atm_api::RESUME_APP_SWITCHES,
            ),
        ] {
            for code in [stop, resume] {
                let mut p = Parcel::new();
                p.write_interface_token(descriptor);
                invoke(service, code, &p, 1000).unwrap();
                assert_eq!(
                    f.host.tasks.app_switches.lock().unwrap().allows(
                        10123,
                        true,
                        false,
                        Instant::now()
                    ),
                    code == resume
                );
            }
        }
        f.host.set_app_switches(1000, true).unwrap();
        set(10123, 0, 1000).unwrap();
        assert_eq!(
            f.host
                .background_activity_start_allowed(10123)
                .unwrap_err()
                .message,
            "no permissionmgr"
        );
        f.host.tasks.app_switches.lock().unwrap().clear_allowances();
        assert!(!allows(10123));
    }

    #[test]
    fn node_query_transform_tracks_window_frames_and_removal() {
        // AbstractAccessibilityServiceConnection.findAccessibilityNodeInfoByAccessibilityId
        // passes this display-space matrix to the original ViewRootImpl connection.
        let f = super::super::focus_tests::Fixture::new();
        let service = Host(f.host.clone());
        let query = |token, uid| {
            let mut p = Parcel::new();
            api::GetWindowTransformationMatrix { token }.write(&mut p);
            let reply = service
                .transact(&mut Call {
                    code: api::GET_WINDOW_TRANSFORMATION_MATRIX,
                    flags: 0,
                    sender_pid: 98201,
                    sender_euid: uid,
                    data: p.reader(),
                })
                .unwrap();
            api::read_get_window_transformation_matrix_reply(&mut reply.reader()).unwrap()
        };
        let identity = vec![1., 0., 0., 0., 1., 0., 0., 0., 1.];
        let token = Some(Binder::Handle(1));
        assert!(query(token, 10123).is_err());
        for absent in [None, token, Some(Binder::Local(1))] {
            assert_eq!(query(absent, 1000).unwrap(), Some(identity.clone()));
        }
        f.window(1, None, false);
        f.window(2, None, false);
        for (left, top) in [(35, 71), (-18, 24), (1280, 640)] {
            let mut windows = f.host.windows.windows.lock().unwrap();
            let w = windows.get_mut(&1).unwrap();
            w.frames.frame = Rect {
                left,
                top,
                right: left + 400,
                bottom: top + 800,
            };
            w.attrs.surface_insets = [7, 9, 11, 13];
            // Child frames are already absolute; neither parent nor insets add twice.
            w.parent = Some(2);
            windows.get_mut(&2).unwrap().frames.frame = Rect {
                left: 100,
                top: 200,
                right: 900,
                bottom: 1000,
            };
            drop(windows);
            assert_eq!(
                query(token, 1000).unwrap(),
                Some(vec![1., 0., left as f32, 0., 1., top as f32, 0., 0., 1.,])
            );
        }
        f.host.remove_window(1);
        assert_eq!(query(token, 1000).unwrap(), Some(identity));
    }
    #[test]
    fn interaction_window_lookups_and_metadata_lifecycle() {
        // WMS.LocalService at android-16.0.0_r1; AccessibilityWindowManagerTest's
        // add/remove connection tests require display lookup and surface metadata.
        let f = super::super::focus_tests::Fixture::new();
        let service = Host(f.host.clone());
        let query = |code, token| {
            let mut p = Parcel::new();
            if code == api::GET_WINDOW_OWNER_USER_ID {
                api::GetWindowOwnerUserId { token }.write(&mut p);
            } else {
                api::GetDisplayIdForWindow { token }.write(&mut p);
            }
            let reply = service
                .transact(&mut Call {
                    code,
                    flags: 0,
                    sender_pid: 98201,
                    sender_euid: 1000,
                    data: p.reader(),
                })
                .unwrap();
            let mut r = reply.reader();
            r.read_exception().unwrap().unwrap();
            r.read_i32().unwrap()
        };
        let token = Some(Binder::Handle(1));
        for absent in [None, token, Some(Binder::Local(1))] {
            assert_eq!(query(api::GET_WINDOW_OWNER_USER_ID, absent), -10000);
            assert_eq!(query(api::GET_DISPLAY_ID_FOR_WINDOW, absent), -1);
        }
        let focus = || {
            let mut p = Parcel::new();
            api::GetFocusedWindowTokenFromWindowStates {}.write(&mut p);
            let reply = service
                .transact(&mut Call {
                    code: api::GET_FOCUSED_WINDOW_TOKEN_FROM_WINDOW_STATES,
                    flags: 0,
                    sender_pid: 98201,
                    sender_euid: 1000,
                    data: p.reader(),
                })
                .unwrap();
            api::read_get_focused_window_token_from_window_states_reply(&mut reply.reader())
                .unwrap()
                .unwrap()
        };
        assert_eq!(focus(), None);
        f.window(1, None, false);
        {
            let mut windows = f.host.windows.windows.lock().unwrap();
            let w = windows.get_mut(&1).unwrap();
            w.uid = 210123;
            f.host.windows.focus.lock().unwrap().current = Some((w.id, w.focus_callbacks.clone()));
        }
        assert_eq!(query(api::GET_WINDOW_OWNER_USER_ID, token), 2);
        assert_eq!(query(api::GET_DISPLAY_ID_FOR_WINDOW, token), 0);
        assert_eq!(focus(), Some(f.observer.binder()));
        let metadata = |token, accessibility_window_id, uid| {
            let mut p = Parcel::new();
            api::SetAccessibilityIdToSurfaceMetadata {
                token,
                accessibility_window_id,
            }
            .write(&mut p);
            invoke(
                &service,
                api::SET_ACCESSIBILITY_ID_TO_SURFACE_METADATA,
                &p,
                uid,
            )
        };
        assert!(metadata(token, 42, 10123).is_err());
        for id in [42, -1, 43] {
            metadata(token, id, 1000).unwrap();
            assert_eq!(
                f.host.windows.windows.lock().unwrap()[&1].accessibility_id,
                id
            );
        }
        struct Surface(
            std::sync::mpsc::Sender<Vec<u8>>,
            Arc<std::sync::atomic::AtomicBool>,
        );
        impl Service for Surface {
            fn descriptor(&self) -> &str {
                "android.ui.ISurfaceComposer"
            }
            fn transact(&self, call: &mut Call<'_>) -> Reply {
                assert_eq!(call.code, 8);
                call.data.enforce_interface(self.descriptor()).unwrap();
                let start = call.data.position();
                call.data.skip(call.data.remaining()).unwrap();
                self.0.send(call.data.since(start).0.to_vec()).unwrap();
                if self.1.load(std::sync::atomic::Ordering::Relaxed) {
                    return reply(Err(Exception::illegal_argument("surface rejected")));
                }
                let mut p = Parcel::new();
                p.write_no_exception();
                Ok(p)
            }
        }
        let (send, receive) = std::sync::mpsc::channel();
        let fail = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let Binder::Local(ptr) = f.remote.add_service(Arc::new(Surface(send, fail.clone()))) else {
            unreachable!()
        };
        let mut p = Parcel::new();
        p.write_i64(ptr as i64);
        let received = f.host.process.transact(0, 1, &p, false).unwrap();
        let remote = received
            .retain_remote_binder(received.reader().read_binder().unwrap().unwrap())
            .unwrap();
        assert!(
            f.host
                .composer
                .set(surface::Composer::for_test(&f.host.process, remote.handle))
                .is_ok()
        );
        f.host
            .windows
            .windows
            .lock()
            .unwrap()
            .get_mut(&1)
            .unwrap()
            .container = Some(surface::Layer::animation_test_layer(
            f.host.process.strong(f.observer.handle),
            1,
        ));
        for id in [44, -1] {
            metadata(token, id, 1000).unwrap();
            let bytes = receive.recv_timeout(Duration::from_secs(2)).unwrap();
            let entry: Vec<_> = [1i32, 1, surface::METADATA_ACCESSIBILITY_ID, 4, id]
                .into_iter()
                .flat_map(i32::to_le_bytes)
                .collect();
            assert!(bytes.windows(entry.len()).any(|w| w == entry));
        }
        fail.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(metadata(token, 45, 1000).is_err());
        assert_eq!(
            f.host.windows.windows.lock().unwrap()[&1].accessibility_id,
            -1
        );
        // A dead/removed window is absent, and late unregister is harmless in WMS.
        f.host.remove_window(1);
        assert_eq!(focus(), None);
        assert_eq!(query(api::GET_WINDOW_OWNER_USER_ID, token), -10000);
        assert_eq!(query(api::GET_DISPLAY_ID_FOR_WINDOW, token), -1);
        metadata(token, -1, 1000).unwrap();
        assert!(f.host.windows.windows.lock().unwrap().is_empty());
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
        // The add's oneway callback must finish its reentrant lookups before
        // this independent edit takes the window lock.
        assert!(next().is_empty());
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
        // Report the visible window snapshot, then its removal.
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
