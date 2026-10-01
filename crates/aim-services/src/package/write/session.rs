//! The original's install sessions, as system_server's bridge tells the
//! write model of them (`PackageWrites` in java/device-services, over
//! `IPackageWritesHost`): each session's parameters whenever they may
//! have changed, sent before the installer commits it, and its end. An
//! install found in the feed is joined to the session that made it: the
//! session's parameters are the install's input the state does not hold
//! (its users, install reason, installer, flags).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, Weak};

use aim_binder_host::local::{Call, LocalProcess, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Binder, Parcel, Reader, UNKNOWN_TRANSACTION};
use aim_service_aidl::{
    dev_aim_server_ibridge as bridge, dev_aim_server_ipackagewriteshost as host,
};

use crate::SYSTEM_UID;
use crate::system::System;

/// How many ended sessions are kept for the installs still to join them.
const ENDED: usize = 16;

/// `PackageInstaller.SessionInfo`'s parameters, as `PackageWrites.send`
/// writes them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Session {
    pub id: i32,
    /// The package, once the session knows it (`getAppPackageName`).
    pub package: Option<String>,
    /// `UserHandle.USER_ALL` (-1) for every user.
    pub user: i32,
    pub mode: i32,
    pub install_flags: i32,
    pub install_reason: i32,
    pub installer: Option<String>,
    pub installer_uid: i32,
    pub originating_uid: i32,
    pub package_source: i32,
    pub enabled_setting_persistent: bool,
    pub multi_package: bool,
    pub staged: bool,
    pub parent: i32,
    pub committed: bool,
    pub base_apk: Option<String>,
    /// Whether its install committed, once it ended.
    pub succeeded: Option<bool>,
}

impl Session {
    pub(super) fn read(id: i32, bytes: &[u8]) -> aim_binder_host::parcel::Result<Session> {
        let mut r = Reader::new(bytes, &[]);
        Ok(Session {
            id,
            package: r.read_string16()?,
            user: r.read_i32()?,
            mode: r.read_i32()?,
            install_flags: r.read_i32()?,
            install_reason: r.read_i32()?,
            installer: r.read_string16()?,
            installer_uid: r.read_i32()?,
            originating_uid: r.read_i32()?,
            package_source: r.read_i32()?,
            enabled_setting_persistent: r.read_bool()?,
            multi_package: r.read_bool()?,
            staged: r.read_bool()?,
            parent: r.read_i32()?,
            committed: r.read_bool()?,
            base_apk: r.read_string16()?,
            succeeded: None,
        })
    }
}

/// The sessions the bridge told of: those open, and the latest ended.
pub struct Sessions {
    process: Arc<LocalProcess>,
    /// This host's `IPackageWritesHost` node.
    node: Binder,
    sessions: Mutex<BTreeMap<i32, Session>>,
}

impl Sessions {
    /// Watches the sessions of each bridge `system` is handed from now on.
    pub fn start(system: &Arc<System>) -> Arc<Sessions> {
        let process = system.process();
        let sessions = Arc::new_cyclic(|this: &Weak<Sessions>| Sessions {
            node: process.add_service(Arc::new(HostNode {
                sessions: this.clone(),
            })),
            process: process.clone(),
            sessions: Mutex::default(),
        });
        let watching = sessions.clone();
        system.add_bridge_listener(Box::new(move |handle| watching.attach(handle)));
        sessions
    }

    /// `IBridge.watchPackageWrites`.
    fn attach(&self, handle: u32) {
        let mut data = Parcel::new();
        bridge::WatchPackageWrites {
            host: Some(self.node),
        }
        .write(&mut data);
        if let Err(s) = self
            .process
            .transact(handle, bridge::WATCH_PACKAGE_WRITES, &data, false)
        {
            eprintln!("package writes: watchPackageWrites: status {s}");
        }
    }

    /// The session whose install committed `package`: the latest ended
    /// one that succeeded for it.
    pub fn installed(&self, package: &str) -> Option<Session> {
        let sessions = self.sessions.lock().unwrap();
        sessions
            .values()
            .rev()
            .find(|s| s.succeeded == Some(true) && s.package.as_deref() == Some(package))
            .cloned()
    }

    fn update(&self, session: Session) {
        let mut sessions = self.sessions.lock().unwrap();
        let succeeded = sessions.get(&session.id).and_then(|s| s.succeeded);
        sessions.insert(
            session.id,
            Session {
                succeeded,
                ..session
            },
        );
    }

    fn finished(&self, id: i32, success: bool) {
        let mut sessions = self.sessions.lock().unwrap();
        if let Some(s) = sessions.get_mut(&id) {
            s.succeeded = Some(success);
        }
        let ended: Vec<i32> = sessions
            .values()
            .filter(|s| s.succeeded.is_some())
            .map(|s| s.id)
            .collect();
        for id in ended.iter().take(ended.len().saturating_sub(ENDED)) {
            sessions.remove(id);
        }
    }
}

/// `IPackageWritesHost`, called by system_server's bridge only.
struct HostNode {
    sessions: Weak<Sessions>,
}

impl Service for HostNode {
    fn descriptor(&self) -> &str {
        host::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.sender_euid != SYSTEM_UID {
            return Err(UNKNOWN_TRANSACTION);
        }
        let Some(sessions) = self.sessions.upgrade() else {
            return Err(UNKNOWN_TRANSACTION);
        };
        let mut reply = Parcel::new();
        match call.code {
            host::SESSION => {
                let a = host::Session::read(&mut call.data)?;
                let info = a.info.ok_or(BAD_VALUE)?;
                sessions.update(Session::read(a.session_id, &info)?);
                host::write_session_reply(&mut reply);
            }
            host::FINISHED => {
                let a = host::Finished::read(&mut call.data)?;
                sessions.finished(a.session_id, a.success);
                host::write_finished_reply(&mut reply);
            }
            _ => return Err(UNKNOWN_TRANSACTION),
        }
        Ok(reply)
    }
}
