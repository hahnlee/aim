//! PackageItemInfo's presentation step stays in the original framework.jar.
use super::AppHost;
use aim_binder_host::parcel::{Binder, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::dev_aim_server_isafelabels as labels;
use std::sync::Arc;

impl AppHost {
    pub(super) fn attach_safe_labels(&self, owner: Option<Binder>) -> Result<(), Exception> {
        let Some(Binder::Handle(handle)) = owner else {
            return Err(Exception::illegal_argument("no safe label bridge"));
        };
        let owner = Arc::new(self.process.strong(handle));
        let weak = self.me.clone();
        self.process.link_to_death(
            &owner,
            Box::new(move || {
                if let Some(host) = weak.upgrade() {
                    let mut current = host.safe_labels.lock().unwrap();
                    if current.as_ref().is_some_and(|p| p.handle == handle) {
                        current.take();
                    }
                }
            }),
        );
        *self.safe_labels.lock().unwrap() = Some(owner);
        Ok(())
    }

    pub(super) fn safe_label(&self, label: &str) -> Result<String, Exception> {
        let owner = self.safe_labels.lock().unwrap().clone().ok_or_else(|| {
            self.unimplemented_named("package", "safe label bridge is not attached (#1855)")
        })?;
        let mut data = Parcel::new();
        labels::MakeSafeLabel {
            label: Some(label.into()),
        }
        .write(&mut data);
        let transport = |status| {
            Exception::new(
                EX_ILLEGAL_STATE,
                format!("safe label: binder status {status}"),
            )
        };
        let reply = owner
            .transact(labels::MAKE_SAFE_LABEL, &data, false)
            .map_err(transport)?;
        labels::read_make_safe_label_reply(&mut reply.reader())
            .map_err(transport)??
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "safe label bridge returned null"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::local::{Call, Reply, Service};
    use aim_binder_host::parcel::{EX_UNSUPPORTED_OPERATION, UNKNOWN_TRANSACTION};
    use std::sync::Mutex;

    struct Original {
        calls: Arc<Mutex<Vec<String>>>,
    }
    impl Service for Original {
        fn descriptor(&self) -> &str {
            labels::DESCRIPTOR
        }
        fn transact(&self, call: &mut Call<'_>) -> Reply {
            assert_eq!(call.sender_euid, super::super::SYSTEM_UID);
            assert_eq!(call.code, labels::MAKE_SAFE_LABEL);
            assert_eq!(call.flags & 1, 0);
            let a = labels::MakeSafeLabel::read(&mut call.data).unwrap();
            let label = a.label.unwrap();
            self.calls.lock().unwrap().push(label.clone());
            if label == "transport" {
                return Err(UNKNOWN_TRANSACTION);
            }
            if label == "exception" {
                let mut p = Parcel::new();
                p.write_exception(&Exception::security("original owner denied"));
                return Ok(p);
            }
            let mut p = Parcel::new();
            labels::write_make_safe_label_reply(
                &mut p,
                &(label != "null").then(|| if label.is_empty() { "" } else { "original…" }.into()),
            );
            Ok(p)
        }
    }

    #[test]
    fn safe_labels_forward_all_text_to_the_original_and_preserve_failures() {
        crate::system::with_discovery_binders(|host, _, _, export| {
            let error = host.safe_label("Clock").unwrap_err();
            assert_eq!(error.code, EX_UNSUPPORTED_OPERATION);
            assert!(error.message.contains("#1855"));
            for owner in [None, Some(Binder::Local(1))] {
                assert!(host.attach_safe_labels(owner).is_err());
            }
            let calls = Arc::new(Mutex::new(Vec::new()));
            let owner = export(Arc::new(Original {
                calls: calls.clone(),
            }));
            host.attach_safe_labels(Some(Binder::Handle(owner.handle)))
                .unwrap();
            let long = "a".repeat(1100);
            let inputs = [
                "Clock",
                "  <b>a&amp;b</b><br>c  ",
                "a\nb\u{2028}c",
                "🙂",
                &long,
            ];
            for label in inputs {
                assert_eq!(host.safe_label(label).unwrap(), "original…");
            }
            assert_eq!(*calls.lock().unwrap(), inputs);
            assert_eq!(host.safe_label("").unwrap(), "");
            assert_eq!(
                host.safe_label("exception").unwrap_err(),
                Exception::security("original owner denied")
            );
            assert!(
                host.safe_label("null")
                    .unwrap_err()
                    .message
                    .contains("returned null")
            );
            assert!(
                host.safe_label("transport")
                    .unwrap_err()
                    .message
                    .contains("binder status")
            );
        });
    }

    #[test]
    fn safe_label_attachment_requires_the_system_uid() {
        use aim_service_aidl::dev_aim_server_isystemprocess as sp;
        let host = AppHost::new(crate::test_process(super::super::SYSTEM_UID));
        let service = super::super::system_process::SystemProcess::new(host.clone());
        let mut p = Parcel::new();
        sp::AttachSystemProcess {
            application_thread: Some(Binder::Handle(1)),
            foreground_service_policy: Some(Binder::Handle(2)),
            safe_labels: Some(Binder::Handle(3)),
        }
        .write(&mut p);
        for uid in [0, 2000, 10100] {
            let reply = service
                .transact(&mut Call {
                    code: sp::ATTACH_SYSTEM_PROCESS,
                    flags: 0,
                    sender_pid: 42,
                    sender_euid: uid,
                    data: p.reader(),
                })
                .unwrap();
            assert_eq!(
                reply.reader().read_exception().unwrap().unwrap_err().code,
                aim_binder_host::parcel::EX_SECURITY
            );
            assert!(host.safe_labels.lock().unwrap().is_none());
        }
    }
}
