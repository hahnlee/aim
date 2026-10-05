//! Binder query endpoints for M4 C. They consume an owned scan snapshot,
//! never the original PMS's shadow feed. These endpoints are not yet
//! registered by guest-init: native scanning, mutation side effects and
//! the SystemServer facade must pass the C gates first (#798).

use std::sync::Arc;
#[cfg(test)]
use std::sync::RwLock;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{EX_UNSUPPORTED_OPERATION, Exception, Parcel, UNKNOWN_TRANSACTION};
use aim_service_aidl::android_content_pm_ipackagemanager as pm;
use aim_service_aidl::android_content_pm_ipackagemanagernative as native;

use super::apps_filter::NotModelled;
#[cfg(test)]
use super::model::State;
use super::query::Query;
use super::resolve::Resolver;

/// Both interfaces share the owner's published state. Each transaction
/// captures one immutable snapshot; publication does not invalidate
/// Binder references already held by clients.
pub struct PackageQueries {
    source: Source,
    resolver: Arc<Resolver>,
    native: bool,
}

enum Source {
    Native(
        Arc<
            dyn Fn() -> Result<Arc<super::scan_snapshot::query_state::Capture>, Exception>
                + Send
                + Sync,
        >,
    ),
    #[cfg(test)]
    Fixture(Arc<RwLock<Arc<State>>>),
}
impl PackageQueries {
    pub fn from_system(system: &Arc<crate::system::System>) -> (Arc<Self>, Arc<Self>) {
        let system = Arc::downgrade(system);
        let source = Arc::new(move || {
            system
                .upgrade()
                .ok_or_else(|| {
                    Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        "native system owner is unavailable",
                    )
                })?
                .capture_package_queries()
        });
        let resolver = Arc::new(Resolver::default());
        (
            Arc::new(Self {
                source: Source::Native(source.clone()),
                resolver: resolver.clone(),
                native: false,
            }),
            Arc::new(Self {
                source: Source::Native(source),
                resolver,
                native: true,
            }),
        )
    }
    #[cfg(test)]
    pub fn new(state: Arc<RwLock<Arc<State>>>) -> (Arc<Self>, Arc<Self>) {
        let resolver = Arc::new(Resolver::default());
        let package = Arc::new(Self {
            source: Source::Fixture(state.clone()),
            resolver: resolver.clone(),
            native: false,
        });
        let package_native = Arc::new(Self {
            source: Source::Fixture(state),
            resolver,
            native: true,
        });
        (package, package_native)
    }
}

impl Service for PackageQueries {
    fn descriptor(&self) -> &str {
        if self.native {
            native::DESCRIPTOR
        } else {
            pm::DESCRIPTOR
        }
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let methods = if self.native {
            native::METHODS
        } else {
            pm::METHODS
        };
        if !methods.iter().any(|(code, _)| *code == call.code) {
            return Err(UNKNOWN_TRANSACTION);
        }
        // Check before dispatch so an unsupported method cannot accept
        // another interface's token. Generated readers check it again.
        let position = call.data.position();
        call.data.enforce_interface(self.descriptor())?;
        call.data.set_position(position);
        let uid = call.sender_euid as i32;
        let capture = match &self.source {
            Source::Native(source) => match source() {
                Ok(capture) => Some(capture),
                Err(error) => {
                    let mut reply = Parcel::new();
                    reply.write_exception(&error);
                    return Ok(reply);
                }
            },
            #[cfg(test)]
            Source::Fixture(_) => None,
        };
        let state = match (&self.source, &capture) {
            (Source::Native(_), Some(capture)) => capture.state().clone(),
            #[cfg(test)]
            (Source::Fixture(state), _) => state.read().unwrap().clone(),
            _ => unreachable!(),
        };
        let resolved = (!self.native)
            .then(|| self.resolver.query(&state, call.code, uid, &mut call.data))
            .flatten();
        let answer = resolved.unwrap_or_else(|| {
            let resolution = match self.resolver.resolution(&state) {
                Ok(resolution) => resolution,
                Err(error) => return Ok(error.reply()),
            };
            Query {
                state: &state,
                filter: &resolution.apps_filter,
                calling_uid: uid,
            }
            .answer(self.descriptor(), call.code, &mut call.data)
        });
        Ok(match answer {
            Ok(reply) => reply,
            Err(NotModelled(reason)) => {
                let mut reply = Parcel::new();
                reply.write_exception(&Exception::new(EX_UNSUPPORTED_OPERATION, reason));
                reply
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::model::{PackageState, PackageUserState, User};
    use super::*;
    use aim_binder_host::parcel::{EX_ILLEGAL_ARGUMENT, Reader};

    fn endpoints() -> (Arc<PackageQueries>, Arc<PackageQueries>) {
        let state = State {
            packages: [(
                "example.app".into(),
                PackageState {
                    name: "example.app".into(),
                    app_id: 10100,
                    users: [(
                        0,
                        PackageUserState {
                            enabled: 2,
                            ..Default::default()
                        },
                    )]
                    .into(),
                    ..Default::default()
                },
            )]
            .into(),
            users: [(
                0,
                User {
                    id: 0,
                    ..Default::default()
                },
            )]
            .into(),
            ..Default::default()
        };
        PackageQueries::new(Arc::new(RwLock::new(Arc::new(state))))
    }

    fn call(service: &PackageQueries, code: u32, uid: u32, data: &Parcel) -> Reply {
        service.transact(&mut Call {
            code,
            flags: 0,
            sender_pid: 77,
            sender_euid: uid,
            data: Reader::new(data.data(), &[]),
        })
    }

    #[test]
    fn binder_identity_controls_package_visibility() {
        let (package, _) = endpoints();
        let mut args = Parcel::new();
        args.write_interface_token(pm::DESCRIPTOR);
        args.write_string16(Some("example.app"));
        args.write_i32(0);
        let reply = call(&package, pm::GET_APPLICATION_ENABLED_SETTING, 10100, &args).unwrap();
        let mut read = Reader::new(reply.data(), &[]);
        assert!(read.read_exception().unwrap().is_ok());
        assert_eq!(read.read_i32().unwrap(), 2);
        let reply = call(&package, pm::GET_APPLICATION_ENABLED_SETTING, 10101, &args).unwrap();
        assert_eq!(
            Reader::new(reply.data(), &[])
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
            EX_ILLEGAL_ARGUMENT
        );
    }

    #[test]
    fn endpoints_enforce_their_own_generated_interface() {
        let (package, package_native) = endpoints();
        assert_eq!(package.descriptor(), pm::DESCRIPTOR);
        assert_eq!(package_native.descriptor(), native::DESCRIPTOR);
        let mut args = Parcel::new();
        args.write_interface_token(pm::DESCRIPTOR);
        assert!(call(&package_native, native::GET_NAMES_FOR_UIDS, 0, &args).is_err());
        assert_eq!(
            call(&package, u32::MAX, 0, &args).unwrap_err(),
            UNKNOWN_TRANSACTION
        );
        let reply = call(&package, pm::SET_APPLICATION_ENABLED_SETTING, 10100, &args).unwrap();
        assert_eq!(
            Reader::new(reply.data(), &[])
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
            EX_UNSUPPORTED_OPERATION
        );
    }

    #[test]
    fn existing_binder_endpoints_follow_owner_publication() {
        let (package, package_native) = endpoints();
        let mut names = Parcel::new();
        names.write_interface_token(native::DESCRIPTOR);
        names.write_i32(1);
        names.write_i32(10100);
        let name = || {
            let reply = call(&package_native, native::GET_NAMES_FOR_UIDS, 0, &names).unwrap();
            let mut read = Reader::new(reply.data(), &[]);
            assert!(read.read_exception().unwrap().is_ok());
            assert_eq!(read.read_i32().unwrap(), 1);
            read.read_string16().unwrap()
        };
        assert_eq!(name().as_deref(), Some("example.app"));
        let Source::Fixture(state) = &package.source else {
            unreachable!()
        };
        let mut next = (**state.read().unwrap()).clone();
        next.packages.clear();
        *state.write().unwrap() = Arc::new(next);
        assert_eq!(name().as_deref(), Some(""));
        let mut enabled = Parcel::new();
        enabled.write_interface_token(pm::DESCRIPTOR);
        enabled.write_string16(Some("example.app"));
        enabled.write_i32(0);
        let reply = call(&package, pm::GET_APPLICATION_ENABLED_SETTING, 0, &enabled).unwrap();
        assert_eq!(
            Reader::new(reply.data(), &[])
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
            EX_ILLEGAL_ARGUMENT
        );
    }
}
