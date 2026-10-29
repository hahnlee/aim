//! Same-task collection, original-Java sampling boundary and native compositor submissions.
use super::super::activity_stack::Activity;
use super::super::activity_start::StartRequest;
use super::super::config::Rect;
use super::super::kept::RawBundle;
use super::super::surface;
use super::*;
use crate::bundle::Value;
use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::Reader;
use aim_host_display::wire;
use aim_hostcall::display::{Connect, Event, event};
use aim_service_aidl::ReadParcelable;
use std::os::fd::AsFd;
use std::os::unix::net::UnixListener;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

fn receive<T>(rx: &Receiver<T>) -> T {
    rx.recv_timeout(Duration::from_secs(5)).unwrap()
}
fn activity(host: &Arc<AppHost>) -> u64 {
    let info = host.activity_info_of("p", "p.Main").unwrap();
    let token = host.new_activity(
        1,
        10100,
        "p",
        info,
        Default::default(),
        7,
        StartRequest::launcher("p.Main"),
    );
    host.activities
        .stack
        .lock()
        .unwrap()
        .activities
        .push(Activity::new(token, 7, 1, "p/p.Main", 0, 0, None));
    token
}
fn options(enter: i32, exit: i32) -> RawBundle {
    let mut p = Parcel::new();
    crate::bundle::write(
        &mut p,
        &[
            ("android:activity.animType", Value::Int(1)),
            (
                "android:activity.packageName",
                Value::String(Some("p".into())),
            ),
            ("android:activity.animEnterRes", Value::Int(enter)),
            ("android:activity.animExitRes", Value::Int(exit)),
        ],
    );
    RawBundle::read_from(&mut p.reader()).unwrap()
}
fn assert_empty(host: &AppHost) {
    assert!(
        host.windows
            .transitions
            .lock()
            .unwrap()
            .collecting
            .is_empty()
    );
}

struct Samples(Sender<(i32, i32)>);
impl Service for Samples {
    fn descriptor(&self) -> &str {
        java::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.code == java::NOTIFY_ANIMATION {
            let a = java::NotifyAnimation::<super::super::kept::RawBundle>::read(&mut call.data)?;
            assert_eq!(
                a.options.unwrap().int("android:activity.animEnterRes", 0)?,
                300
            );
            self.0.send((9, i32::from(a.started))).unwrap();
            return Ok(Parcel::new());
        }
        assert_eq!(call.code, java::SAMPLE);
        let a = java::Sample::read(&mut call.data)?;
        assert_eq!(a.package_name.as_deref(), Some("p"));
        assert_eq!((a.width, a.height), (100, 200));
        self.0.send((a.attribute, a.custom)).unwrap();
        let mut samples = Vec::new();
        for ms in 0..=50 {
            let mut f = IDENTITY;
            f[4] = 50. - ms as f32;
            f[6] = ms as f32 / 50.;
            samples.extend(f);
        }
        let mut p = Parcel::new();
        java::write_sample_reply(&mut p, &Some(samples));
        Ok(p)
    }
}

struct Frames(Sender<Vec<(i32, [f32; 11])>>);
impl Service for Frames {
    fn descriptor(&self) -> &str {
        "android.ui.ISurfaceComposer"
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert_eq!(call.code, 8);
        let start = call.data.position();
        call.data.skip(call.data.remaining())?;
        let (bytes, objects) = call.data.since(start);
        let mut frames = Vec::new();
        // Only the apply token and layer handles are Binder objects in these transactions.
        for &at in &objects[1..] {
            let mut r = Reader::new(bytes, &objects);
            r.set_position(at as usize);
            r.read_binder()?;
            let id = r.read_i32()?;
            let what = r.read_i64()?;
            if what == 0x8000 {
                continue;
            } // Removing the process's layer.
            assert_eq!(what & 0x19, 0x19); // position, alpha, matrix
            let (x, y) = (r.read_f32()?, r.read_f32()?);
            r.skip(16)?;
            let mut f = IDENTITY;
            for v in &mut f[..4] {
                *v = r.read_f32()?;
            }
            f[4] = x;
            f[5] = y;
            let clip = [r.read_f32()?, r.read_f32()?, r.read_f32()?, r.read_f32()?];
            f[7..].copy_from_slice(&[clip[1], clip[0], clip[3], clip[2]]);
            r.skip(20)?;
            f[6] = r.read_f32()?;
            frames.push((id, f));
        }
        self.0.send(frames).unwrap();
        Ok(Parcel::new())
    }
}

#[test]
fn vsync_playback_replacement_and_window_removal_preserve_new_collection() {
    for with_options in [false, true] {
        crate::system::with_discovery_binders(|host, _, _, export| {
            host.tasks.windows.set(true).unwrap();
            host.tasks.animation_test_task(
                7,
                Rect {
                    left: 10,
                    top: 20,
                    right: 110,
                    bottom: 220,
                },
            );
            let token = activity(host);
            let (frames_tx, frames) = mpsc::channel();
            let compositor = export(Arc::new(Frames(frames_tx)));
            assert!(
                host.composer
                    .set(surface::Composer::for_test(
                        &host.process,
                        compositor.handle
                    ))
                    .is_ok()
            );
            host.animation_test_window(export(Arc::new(Samples(mpsc::channel().0))), token, 7, 1);
            host.animation_test_window(export(Arc::new(Samples(mpsc::channel().0))), 2, 7, 2);
            let (sample_tx, samples) = mpsc::channel();
            host.windows.transitions.lock().unwrap().owner =
                Some(Arc::new(export(Arc::new(Samples(sample_tx)))));
            let path = std::path::PathBuf::from(format!(
                "/tmp/aim-anim-{}-{token}.sock",
                std::process::id()
            ));
            let listener = UnixListener::bind(&path).unwrap();
            host.windows.transitions.lock().unwrap().socket = Some(path.clone());
            let (tick, ticks) = mpsc::channel::<Option<i64>>();
            let observer = std::thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let request = wire::recv_record::<wire::Request>(stream.as_fd())
                    .unwrap()
                    .unwrap();
                assert_eq!(request.op, wire::OP_VSYNC_OBSERVER);
                wire::send(stream.as_fd(), wire::bytes(&Connect::default()), None).unwrap();
                assert_eq!(
                    wire::recv_record::<wire::Request>(stream.as_fd())
                        .unwrap()
                        .unwrap()
                        .op,
                    wire::OP_SET_VSYNC
                );
                while let Some(ns) = receive(&ticks) {
                    wire::send(
                        stream.as_fd(),
                        wire::bytes(&Event {
                            kind: event::VSYNC,
                            timestamp_ns: ns,
                            ..Default::default()
                        }),
                        None,
                    )
                    .unwrap();
                }
                // Retirement closes the observer, even if no new vsync arrives.
                assert!(
                    wire::recv_record::<Event>(stream.as_fd())
                        .unwrap()
                        .is_none()
                );
            });
            host.override_activity_transition(
                10100,
                Some(Binder::Local(token)),
                true,
                Some((100, 101, 0)),
            )
            .unwrap();
            if with_options {
                let options = options(300, 301);
                host.activities
                    .stack
                    .lock()
                    .unwrap()
                    .activities
                    .push(Activity::new(2, 7, 1, "p/A", 0, 0, None));
                host.collect_open_with_options(Some(7), Some(&options))
                    .unwrap();
                host.collect_open(Some(7), token);
            } else {
                host.collect_transition(7, Transition::new(2, token, true));
            }
            host.transition_ready(token);
            assert_eq!(receive(&samples), (0, if with_options { 300 } else { 100 }));
            assert_eq!(receive(&samples), (1, if with_options { 301 } else { 101 }));
            if with_options {
                assert_eq!(receive(&samples), (9, 1));
            }
            let old = host.windows.transitions.lock().unwrap().collecting[&7].id;
            for (ns, x, alpha) in [(1_000_000, 50., 0.), (17_666_667, 33.333332, 1. / 3.)] {
                tick.send(Some(ns)).unwrap();
                let f = receive(&frames);
                assert_eq!(f.len(), 2);
                for (_, f) in f {
                    assert!((f[4] - x).abs() < 0.001);
                    assert!((f[6] - alpha).abs() < 0.001);
                }
            }
            host.collect_transition(7, Transition::new(token, 3, true));
            let reset = receive(&frames);
            assert!(reset.iter().all(|(_, f)| *f == IDENTITY));
            if with_options {
                assert_eq!(receive(&samples), (9, 0));
            }
            let new = host.windows.transitions.lock().unwrap().collecting[&7].id;
            assert_ne!(old, new);
            assert!(
                !host
                    .transition_frame(7, old, token, IDENTITY, 2, IDENTITY)
                    .unwrap()
            );
            host.finish_transition(7, old); // The retiring worker cannot remove the new one.
            assert_eq!(
                host.windows.transitions.lock().unwrap().collecting[&7].id,
                new
            );
            host.remove_animation_test_window(1);
            receive(&frames);
            assert_empty(host);
            assert!(
                !host
                    .transition_frame(7, new, 3, IDENTITY, token, IDENTITY)
                    .unwrap()
            );
            host.collect_transition(7, Transition::new(2, 4, false));
            host.windows_died(2);
            receive(&frames); // Reset the surviving participants.
            assert!(receive(&frames).is_empty()); // Remove the dead process's layer.
            assert_empty(host);
            tick.send(None).unwrap();
            observer.join().unwrap();
            std::fs::remove_file(path).unwrap();
        });
    }
}

#[test]
fn override_clear_options_and_pending_priority_are_scoped_to_the_activity() {
    crate::system::with_discovery_session(|host, _, _| {
        host.tasks.windows.set(true).unwrap();
        let token = activity(host);
        let set = |uid, open, custom| {
            host.override_activity_transition(uid, Some(Binder::Local(token)), open, custom)
                .unwrap()
        };
        set(10100, true, Some((100, 101, 0)));
        set(10100, false, Some((200, 201, 0)));
        set(10101, true, None);
        let overrides = || {
            host.activities.records.lock().unwrap()[&token]
                .transition_overrides
                .clone()
        };
        assert_eq!(
            overrides(),
            [Some(("p".into(), 100, 101)), Some(("p".into(), 200, 201))]
        );
        set(10100, true, None);
        assert!(overrides()[0].is_none());
        assert!(overrides()[1].is_some());
        let options = options(300, 0);
        host.collect_open_with_options(Some(7), Some(&options))
            .unwrap();
        host.collect_open(Some(7), 2);
        let custom = || {
            host.windows.transitions.lock().unwrap().collecting[&7]
                .custom
                .clone()
        };
        assert_eq!(custom(), Some(("p".into(), 300, 0))); // Zero explicitly disables the exit.
        host.override_transition(
            10100,
            Some(Binder::Local(token)),
            Some("p".into()),
            400,
            401,
            0,
        )
        .unwrap();
        assert_eq!(custom(), Some(("p".into(), 400, 401)));
        assert_eq!(overrides()[1], Some(("p".into(), 200, 201))); // A pending override never persists.
        host.cancel_activity_transition(token);
        assert_empty(host);
    });
}
