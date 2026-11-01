//! WMS.getCaptureArgs and ScreenCapture's Java/JNI parcel conversion,
//! android-16.0.0_r1. SurfaceFlinger owns composition and the result callback.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::*;
use aim_binder_host::parcel::{Reader, Result as ParcelResult};
use aim_service_aidl::{ReadParcelable, android_gui_isurfacecomposer as sf};

struct CaptureArgs {
    format: i32,
    crop: [i32; 4],
    scale: [f32; 2],
    secure: bool,
    protected: bool,
    uid: i64,
    grayscale: bool,
    excludes: Vec<Binder>,
    seamless: bool,
}
impl Default for CaptureArgs {
    fn default() -> Self {
        Self {
            format: 1,
            crop: [0; 4],
            scale: [1.; 2],
            secure: false,
            protected: false,
            uid: -1,
            grayscale: false,
            excludes: Vec::new(),
            seamless: false,
        }
    }
}
impl ReadParcelable for CaptureArgs {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let mut a = Self {
            format: r.read_i32()?,
            crop: [r.read_i32()?, r.read_i32()?, r.read_i32()?, r.read_i32()?],
            scale: [r.read_f32()?, r.read_f32()?],
            secure: r.read_bool()?,
            protected: r.read_bool()?,
            uid: r.read_i64()?,
            grayscale: r.read_bool()?,
            ..Default::default()
        };
        let count = r.read_i32()?;
        if count > 0 && count as usize > r.remaining() / 16 {
            return Err(BAD_VALUE);
        }
        for _ in 0..count {
            // SurfaceControl.java's header, then SurfaceControl::readFromParcel.
            r.read_string8()?;
            r.read_i32()?;
            r.read_i32()?;
            if !r.read_bool()? {
                return Err(BAD_VALUE);
            }
            r.read_binder()?; // client
            a.excludes.push(r.read_binder()?.ok_or(BAD_VALUE)?);
            r.read_i32()?; // layer id
            r.read_string16()?;
            for _ in 0..4 {
                r.read_i32()?;
            } // transform hint, width, height, format
        }
        a.seamless = r.read_bool()?;
        Ok(a)
    }
}
struct Listener(Option<Binder>);
impl ReadParcelable for Listener {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        Ok(Self(if r.read_bool()? {
            r.read_binder()?
        } else {
            None
        }))
    }
}
impl CaptureArgs {
    fn layers(&self, root: Binder, listener: Option<Binder>) -> Parcel {
        let mut p = Parcel::new();
        p.write_interface_token(sf::DESCRIPTOR);
        p.write_i32(1);
        let layer_start = p.position();
        p.write_i32(0); // sized LayerCaptureArgs
        p.write_i32(1);
        let start = p.position();
        p.write_i32(0); // sized native CaptureArgs
        p.write_i32(self.format);
        p.write_i32(1);
        p.write_i32(20); // sized ARect (left, top, right, bottom)
        self.crop.iter().for_each(|v| p.write_i32(*v));
        self.scale.iter().for_each(|v| p.write_f32(*v));
        p.write_bool(self.secure);
        p.write_i32(self.uid as i32); // JNI's long -> native int32_t
        p.write_i32(0); // dataspace UNKNOWN
        p.write_bool(self.protected);
        p.write_bool(self.grayscale);
        p.write_i32(self.excludes.len() as i32);
        for handle in &self.excludes {
            p.write_binder(Some(*handle));
        }
        p.write_bool(self.seamless);
        p.set_i32_at(start, (p.position() - start) as i32);
        p.write_binder(Some(root));
        p.write_bool(true); // LayerCaptureArgs.Builder's childrenOnly default
        p.set_i32_at(layer_start, (p.position() - layer_start) as i32);
        p.write_binder(listener);
        p
    }
}
impl AppHost {
    /// The logical display's SurfaceControl subtree. Top-level tasks and
    /// non-task windows share this root, as DisplayContent's children do.
    pub(in crate::app_host) fn with_display_layer<T>(
        &self,
        f: impl FnOnce(&Layer) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut root = self.windows.display_layer.lock().unwrap();
        if root.is_none() {
            *root = Some(self.composer().map_err(|e| e.message)?.create_surface(
                &self.process,
                "Display 0",
                surface::CONTAINER,
                None,
                &[],
            )?);
        }
        f(root.as_ref().unwrap())
    }

    pub(super) fn capture_display(&self, call: &mut Call<'_>) -> Reply {
        let a = wm::CaptureDisplay::<CaptureArgs, Listener>::read(&mut call.data)
            .map_err(|_| BAD_VALUE)?;
        let result = (|| {
            if self.check_permission(
                Some("android.permission.READ_FRAME_BUFFER"),
                call.sender_euid as i32,
            )? != 0
            {
                return Err(Exception::security("Requires READ_FRAME_BUFFER permission"));
            }
            if a.display_id != 0 {
                return Err(Exception::illegal_argument(format!(
                    "Trying to screenshot and invalid display: {}",
                    a.display_id
                )));
            }
            let bounds = self.display_metrics()?;
            let mut args = a.capture_args.unwrap_or_default();
            if args.crop[0] >= args.crop[2] || args.crop[1] >= args.crop[3] {
                args.crop = [0, 0, bounds.width, bounds.height];
            }
            self.with_display_layer(|root| {
                Ok(args.layers(root.handle, a.listener.and_then(|l| l.0)))
            })
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))
        })();
        let data = match result {
            Ok(data) => data,
            Err(e) => return reply(Err(e)),
        };
        // captureLayers is oneway: transport errors propagate here; capture
        // errors and GraphicBuffers go directly to the caller's listener.
        self.composer()
            .map_err(|_| BAD_VALUE)?
            .capture_layers(&data)?;
        Ok(Parcel::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    // UiAutomation.takeScreenshot's WMS route: pinned WMS.getCaptureArgs,
    // ScreenCapture.java, android_window_ScreenCapture.cpp and libgui AIDL.
    struct Surface {
        expected: Mutex<Vec<Binder>>,
        crops: mpsc::Sender<([i32; 4], [f32; 2])>,
    }
    impl Service for Surface {
        fn descriptor(&self) -> &str {
            sf::DESCRIPTOR
        }
        fn transact(&self, call: &mut Call<'_>) -> Reply {
            let oneway = call.is_oneway();
            let r = &mut call.data;
            if call.code == 100 {
                let mut expected = self.expected.lock().unwrap();
                for _ in 0..3 {
                    expected.push(r.read_binder()?.unwrap());
                }
                return Ok(Parcel::new());
            }
            let expected = self.expected.lock().unwrap();
            if call.code == 1 {
                r.enforce_interface("android.gui.ISurfaceComposerClient")?;
                assert_eq!(r.read_string16()?.as_deref(), Some("Display 0"));
                assert_eq!(r.read_i32()?, surface::CONTAINER);
                assert_eq!(r.read_binder()?, None);
                let mut p = Parcel::new();
                p.write_no_exception();
                p.write_i32(1);
                p.write_i32(0); // CreateSurfaceResult size is read by create_surface
                p.write_binder(Some(expected[0]));
                p.write_i32(7);
                p.write_string16(Some("Display 0"));
                p.write_i32(0);
                return Ok(p);
            }
            assert_eq!(call.code, 29); // pinned ISurfaceComposer.captureLayers
            assert!(oneway);
            r.enforce_interface(sf::DESCRIPTOR)?;
            assert_eq!(r.read_i32()?, 1);
            let start = r.position();
            let size = r.read_i32()? as usize;
            assert_eq!(r.read_i32()?, 1);
            let args_start = r.position();
            let args_size = r.read_i32()? as usize;
            let custom = r.read_i32()? == 4;
            assert_eq!(r.read_i32()?, 1);
            assert_eq!(r.read_i32()?, 20);
            let crop = [r.read_i32()?, r.read_i32()?, r.read_i32()?, r.read_i32()?];
            let scale = [r.read_f32()?, r.read_f32()?];
            assert_eq!(r.read_bool()?, custom);
            assert_eq!(r.read_i32()?, if custom { 10123 } else { -1 });
            assert_eq!(r.read_i32()?, 0); // dataspace
            assert_eq!(r.read_bool()?, custom);
            assert_eq!(r.read_bool()?, custom);
            assert_eq!(r.read_i32()?, if custom { 2 } else { 0 });
            if custom {
                assert_eq!(r.read_binder()?, Some(expected[2]));
                assert_eq!(r.read_binder()?, Some(expected[0]));
            }
            assert_eq!(r.read_bool()?, custom);
            assert_eq!(r.position(), args_start + args_size);
            assert_eq!(r.read_binder()?, Some(expected[0]));
            assert!(r.read_bool()?); // childrenOnly
            assert_eq!(r.position(), start + size);
            assert_eq!(r.read_binder()?, Some(expected[1]));
            assert_eq!(r.remaining(), 0);
            self.crops.send((crop, scale)).unwrap();
            Ok(Parcel::new())
        }
    }
    fn request(display: i32, custom: bool, listener: Binder, excludes: &[Binder]) -> Parcel {
        let mut p = Parcel::new();
        p.write_interface_token(wm::DESCRIPTOR);
        p.write_i32(display);
        p.write_bool(custom);
        if custom {
            p.write_i32(4); // RGB_565
            [10, 20, 510, 1020].iter().for_each(|v| p.write_i32(*v));
            p.write_f32(0.25);
            p.write_f32(0.5);
            p.write_bool(true);
            p.write_bool(true);
            p.write_i64(10123);
            p.write_bool(true);
            p.write_i32(excludes.len() as i32);
            for handle in excludes {
                p.write_string8(Some("excluded"));
                p.write_i32(99);
                p.write_i32(88);
                p.write_bool(true);
                p.write_binder(Some(listener)); // SurfaceControl's client
                p.write_binder(Some(*handle));
                p.write_i32(17);
                p.write_string16(Some("excluded"));
                [0, 99, 88, 1].iter().for_each(|v| p.write_i32(*v));
            }
            p.write_bool(true);
        }
        p.write_bool(true); // AIDL typed-object presence
        let start = p.position();
        // Original ScreenCaptureListener.writeToParcel, then JNI's writeStrongBinder.
        // Live app_process oracle: listener_body_size=32 native_present=1.
        p.write_bool(true);
        p.write_binder(Some(listener));
        assert_eq!(p.position() - start, 32);
        p
    }
    #[test]
    fn listener_without_native_object_does_not_read_a_binder() {
        let mut p = Parcel::new();
        p.write_bool(false);
        p.write_i32(42); // next field must remain unread
        let mut r = p.reader();
        assert_eq!(Listener::read_from(&mut r).unwrap().0, None);
        assert_eq!(r.read_i32().unwrap(), 42);
        assert_eq!(r.remaining(), 0);
        let mut truncated = Parcel::new();
        truncated.write_bool(true);
        assert!(Listener::read_from(&mut truncated.reader()).is_err());
    }
    fn invoke(host: &Arc<AppHost>, p: &Parcel, uid: u32) -> Reply {
        WindowManager(host.clone()).transact(&mut Call {
            code: wm::CAPTURE_DISPLAY,
            flags: aim_binder_driver::uapi::TF_ONE_WAY,
            sender_pid: 55,
            sender_euid: uid,
            data: p.reader(),
        })
    }
    fn setup(host: &Arc<AppHost>) {
        let permissions = crate::app_host::permission_manager::PermissionManager::with_system(
            host.clone(),
            Default::default(),
        );
        assert!(host.permission_manager.set(Arc::new(permissions)).is_ok());
        assert!(
            host.display_metrics
                .set(Ok(crate::app_host::display::Metrics {
                    width: 1440,
                    height: 2880,
                    density_dpi: 320,
                }))
                .is_ok()
        );
    }
    #[test]
    fn capture_forwards_display_subtree_args_and_listener() {
        crate::system::with_discovery_binders(|host, _, _, export| {
            setup(host);

            let root = export(Arc::new(super::super::super::activity::Token));
            let listener = export(Arc::new(super::super::super::activity::Token));
            let exclude = export(Arc::new(super::super::super::activity::Token));
            let (send, receive) = mpsc::channel();
            let endpoint = export(Arc::new(Surface {
                expected: Mutex::default(),
                crops: send,
            }));
            let mut init = Parcel::new();
            for b in [&root, &listener, &exclude] {
                init.write_binder(Some(b.binder()));
            }
            endpoint.transact(100, &init, false).unwrap();
            assert!(
                host.composer
                    .set(surface::Composer::for_test(&host.process, endpoint.handle))
                    .is_ok()
            );
            for custom in [false, true, false] {
                let p = request(
                    0,
                    custom,
                    listener.binder(),
                    &[exclude.binder(), root.binder()],
                );
                assert!(invoke(host, &p, 1000).unwrap().data().is_empty());
                assert_eq!(
                    receive.recv_timeout(Duration::from_secs(3)).unwrap(),
                    if custom {
                        ([10, 20, 510, 1020], [0.25, 0.5])
                    } else {
                        ([0, 0, 1440, 2880], [1., 1.])
                    }
                );
            }
        });
    }
    #[test]
    fn capture_checks_permission_display_and_propagates_transport_error() {
        crate::system::with_discovery_binders(|host, _, _, export| {
            setup(host);
            let root = export(Arc::new(super::super::super::activity::Token));
            let p = request(-1, false, root.binder(), &[]);
            let denied = invoke(host, &p, 10100).unwrap();
            let e = denied.reader().read_exception().unwrap().unwrap_err();
            assert_eq!(e.code, aim_binder_host::parcel::EX_SECURITY);
            assert_eq!(e.message, "Requires READ_FRAME_BUFFER permission");
            let invalid = invoke(host, &p, 1000).unwrap();
            assert_eq!(
                invalid.reader().read_exception().unwrap().unwrap_err().code,
                aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT
            );
            assert!(host.composer.get().is_none());
            assert!(host.windows.display_layer.lock().unwrap().is_none());

            *host.windows.display_layer.lock().unwrap() =
                Some(surface::Layer::animation_test_layer(root, 7));
            assert!(
                host.composer
                    .set(surface::Composer::for_test(&host.process, u32::MAX))
                    .is_ok()
            );
            let data = request(0, false, Binder::Handle(0), &[]);
            let error = host
                .composer
                .get()
                .unwrap()
                .capture_layers(&Parcel::new())
                .unwrap_err();
            assert_eq!(invoke(host, &data, 1000).err(), Some(error));
        });
    }
}
