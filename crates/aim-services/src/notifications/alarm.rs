//! Java observes AlarmManager's broadcast and attributes its show intent.
use std::sync::{Arc, Weak, mpsc};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{PERMISSION_DENIED, Parcel, UNKNOWN_TRANSACTION};
use aim_host_display::notify::{Indicator, Message};
use aim_service_aidl::{
    dev_aim_server_ialarmindicator as alarm, dev_aim_server_ialarmindicatorlistener as listener,
};

use super::{Bridge, RETRY, SYSTEM_UID};

impl Bridge {
    pub(super) fn follow_alarm(self: Arc<Self>) {
        let callback = self
            .process
            .add_service(Arc::new(Listener(Arc::downgrade(&self))));
        loop {
            let Some(owner) = self.find("aim.alarm_indicator") else {
                std::thread::sleep(RETRY);
                continue;
            };
            let (tx, rx) = mpsc::channel();
            let death = self.process.link_to_death(
                &owner,
                Box::new(move || {
                    if let Err(e) = tx.send(()) {
                        eprintln!("guest-init: next alarm: {e}");
                    }
                }),
            );
            let mut data = Parcel::new();
            alarm::Watch {
                listener: Some(callback),
            }
            .write(&mut data);
            let registered = owner
                .transact(alarm::WATCH, &data, false)
                .map_err(|s| format!("status {s}"))
                .and_then(|reply| {
                    alarm::read_watch_reply(&mut reply.reader())
                        .map_err(|s| format!("status {s}"))?
                        .map_err(|e| e.message)
                });
            match registered {
                Ok(()) => {
                    if let Err(e) = rx.recv() {
                        eprintln!("guest-init: next alarm: {e}");
                    }
                }
                Err(e) => eprintln!("guest-init: next alarm: watch: {e}"),
            }
            self.process.clear_death(&owner, death);
            self.alarm_changed(None);
            std::thread::sleep(RETRY);
        }
    }

    fn alarm_changed(&self, owner: Option<String>) {
        let mut current = self.alarm.lock().unwrap();
        for message in changes(&mut current, owner) {
            self.send(&message);
        }
    }
}

fn changes(current: &mut Option<String>, owner: Option<String>) -> Vec<Message> {
    if *current == owner {
        return Vec::new();
    }
    let old = std::mem::replace(current, owner.clone());
    [(old, false), (owner, true)]
        .into_iter()
        .filter_map(|(package, on)| {
            package.map(|package| Message::Indicator {
                package,
                indicator: Indicator::Alarm,
                on,
            })
        })
        .collect()
}

struct Listener(Weak<Bridge>);
impl Service for Listener {
    fn descriptor(&self) -> &str {
        listener::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.sender_euid != SYSTEM_UID {
            return Err(PERMISSION_DENIED);
        }
        if call.code != listener::CHANGED {
            return Err(UNKNOWN_TRANSACTION);
        }
        let args = listener::Changed::read(&mut call.data)?;
        let bridge = self.0.upgrade().ok_or(UNKNOWN_TRANSACTION)?;
        bridge.alarm_changed(args.package_name);
        Ok(Parcel::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_non_system_callbacks_before_reading_their_payload() {
        let listener = Listener(Weak::new());
        let data = Parcel::new();
        for uid in [0, 2000, 10100] {
            let result = listener.transact(&mut Call {
                code: listener::CHANGED,
                flags: 1,
                sender_pid: 42,
                sender_euid: uid,
                data: data.reader(),
            });
            assert!(matches!(result, Err(PERMISSION_DENIED)));
        }
    }

    #[test]
    fn moves_the_alarm_item_and_clears_it_on_removal_or_owner_death() {
        let mut owner = None;
        let events = |package: &str, on| Message::Indicator {
            package: package.into(),
            indicator: Indicator::Alarm,
            on,
        };
        assert_eq!(changes(&mut owner, Some("a".into())), [events("a", true)]);
        assert!(changes(&mut owner, Some("a".into())).is_empty());
        assert_eq!(
            changes(&mut owner, Some("b".into())),
            [events("a", false), events("b", true)]
        );
        assert_eq!(changes(&mut owner, None), [events("b", false)]);
        assert!(changes(&mut owner, None).is_empty());
    }
}
