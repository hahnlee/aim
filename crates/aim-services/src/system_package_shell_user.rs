//! Original User creation shell execution with real transferred descriptors and caller.
use super::*;
use aim_binder_host::server::RetainedFd;
use aim_service_aidl::{
    WriteParcelable, dev_aim_server_ipackagebootstrapbridge as bootstrap,
    dev_aim_server_ipackageshellpolicybridge as user,
};

struct Descriptor(aim_binder_driver::File);
impl WriteParcelable for Descriptor {
    fn write_to(&self, parcel: &mut Parcel) {
        parcel.write_i32(0);
        parcel.write_file(self.0.clone());
    }
}
fn failure(message: impl Into<String>) -> Exception {
    Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, message)
}
fn read_user_reply(reader: &mut Reader<'_>) -> Result<i32> {
    let result = user::read_handle_create_user_command_reply(reader)
        .map_err(|status| failure(format!("User creation shell reply: {status}")))??;
    if reader.remaining() != 0 {
        return Err(failure("User creation shell reply tail"));
    }
    Ok(result)
}
fn transact_user_command(
    process: &Arc<LocalProcess>,
    owner: &Strong,
    request: &Parcel,
) -> Result<i32> {
    if process.authenticated_inbound_identity().is_none() {
        return Err(Exception::security(
            "User creation shell forwarding requires live inbound provenance",
        ));
    }
    let reply = process
        .transact_preserving_inbound(owner.handle, user::HANDLE_CREATE_USER_COMMAND, request)
        .map_err(|status| failure(format!("User creation shell transport: {status}")))?;
    read_user_reply(&mut reply.reader())
}
impl System {
    pub fn shell_create_user_command(
        &self,
        uid: u32,
        pid: i32,
        input: RetainedFd,
        output: RetainedFd,
        error: RetainedFd,
        arguments: Vec<String>,
    ) -> Result<i32> {
        if self.process.authenticated_inbound_identity() != Some((pid, uid)) {
            return Err(Exception::security(
                "User creation shell caller lacks authenticated Binder provenance",
            ));
        }
        let bridge = self.package_bootstrap()?;
        self.check_package_bootstrap(&bridge)?;
        let owner =
            self.package_bootstrap_binder_leaf(&bridge, bootstrap::GET_PACKAGE_SHELL_POLICY_BRIDGE)?;
        let mut request = Parcel::new();
        user::HandleCreateUserCommand {
            input: Some(Descriptor(input.into_file_owner())),
            output: Some(Descriptor(output.into_file_owner())),
            error: Some(Descriptor(error.into_file_owner())),
            arguments: Some(arguments.into_iter().map(Some).collect()),
        }
        .write(&mut request);
        // The original UserManager checks Binder.getCallingUid itself.
        // This forwards only the actual kernel-authenticated inbound identity.
        let result = transact_user_command(&self.process, &owner, &request)?;
        self.check_package_bootstrap(&bridge)?;
        Ok(result)
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::{
        local::{Call, LocalProcess, Reply, Service},
        parcel::BAD_VALUE,
    };
    use aim_service_aidl::ReadParcelable;
    use std::{
        io::{Read, Seek, SeekFrom, Write},
        os::fd::AsFd,
        sync::{Arc, Mutex, Weak},
    };
    struct IncomingFd(u32);
    impl ReadParcelable for IncomingFd {
        fn read_from(reader: &mut Reader<'_>) -> aim_binder_host::parcel::Result<Self> {
            if reader.read_i32()? != 0 {
                return Err(BAD_VALUE);
            }
            Ok(Self(reader.read_fd()?))
        }
    }
    struct TransportOwner {
        process: Weak<LocalProcess>,
        held: Arc<Mutex<Option<RetainedFd>>>,
    }
    impl Service for TransportOwner {
        fn descriptor(&self) -> &str {
            user::DESCRIPTOR
        }
        fn accepts_fds(&self) -> bool {
            true
        }
        fn transact(&self, call: &mut Call<'_>) -> Reply {
            assert_eq!(call.code, user::HANDLE_CREATE_USER_COMMAND);
            let request = user::HandleCreateUserCommand::<IncomingFd>::read(&mut call.data)?;
            assert_eq!(call.data.remaining(), 0);
            assert_eq!(
                request.arguments,
                Some(vec![
                    Some("create-user".into()),
                    Some("fixture".into())
                ])
            );
            let process = self.process.upgrade().unwrap();
            for fd in [request.input.unwrap().0, request.error.unwrap().0] {
                assert!(process.file(fd).is_some());
            }
            let file = process.file(request.output.unwrap().0).unwrap();
            let mut output = aim_binder_host::server::file_fd(&file).unwrap();
            output.write_all(b"transport-only").unwrap();
            *self.held.lock().unwrap() = Some(output);
            let mut reply = Parcel::new();
            user::write_handle_create_user_command_reply(&mut reply, -17);
            Ok(reply)
        }
    }
    #[test]
    fn actual_descriptor_roundtrip_retains_owner_after_parcel_and_sender_close() {
        let driver = aim_binder_driver::Driver::new();
        let process = LocalProcess::open(
            &driver,
            aim_binder_driver::Device::Binder,
            aim_binder_driver::Credentials {
                pid: 190412,
                euid: 1000,
                security_context: None,
            },
        );
        let path = std::env::temp_dir().join(format!("aim-create-user-shell-fd-{}", std::process::id()));
        let output = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let file = aim_binder_host::server::file_from_fd(output.as_fd()).unwrap();
        let mut request = Parcel::new();
        user::HandleCreateUserCommand {
            input: Some(Descriptor(file.clone())),
            output: Some(Descriptor(file.clone())),
            error: Some(Descriptor(file)),
            arguments: Some(vec![
                Some("create-user".into()),
                Some("fixture".into()),
            ]),
        }
        .write(&mut request);
        let held = Arc::new(Mutex::new(None));
        let Binder::Local(ptr) = process.add_service(Arc::new(TransportOwner {
            process: Arc::downgrade(&process),
            held: held.clone(),
        })) else {
            panic!("local node")
        };
        let received = process
            .local_service(ptr)
            .unwrap()
            .transact(user::HANDLE_CREATE_USER_COMMAND, &request, false)
            .unwrap();
        assert_eq!(read_user_reply(&mut received.reader()).unwrap(), -17);
        drop(received);
        drop(request);
        drop(output);
        let mut retained = held.lock().unwrap().take().unwrap();
        retained.seek(SeekFrom::Start(0)).unwrap();
        let mut bytes = Vec::new();
        retained.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"transport-only");
        drop(retained);
        driver.release(process.proc_handle());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn user_reply_propagates_original_exception_and_rejects_tail() {
        let mut reply = Parcel::new();
        reply.write_exception(&Exception::security("original caller denied"));
        let error = read_user_reply(&mut reply.reader()).unwrap_err();
        assert_eq!(error.code, aim_binder_host::parcel::EX_SECURITY);
        assert_eq!(error.message, "original caller denied");
        let mut reply = Parcel::new();
        user::write_handle_create_user_command_reply(&mut reply, 0);
        reply.write_i32(7);
        assert_eq!(
            read_user_reply(&mut reply.reader()).unwrap_err().message,
            "User creation shell reply tail"
        );
        let mut reply = Parcel::new();
        reply.write_no_exception();
        assert!(read_user_reply(&mut reply.reader()).is_err());
    }
    #[test]
    fn real_inbound_profile_forwarding_preserves_shell_identity_and_rejects_synthetic_calls() {
        use aim_binder_driver::{Credentials, Device, Driver, Errno, File, GuestProcess, uapi::*};
        use std::collections::HashMap;
        struct NoUserMemory;
        impl GuestProcess for NoUserMemory {
            fn copy_from_user(&mut self, _: u64, _: &mut [u8]) -> std::result::Result<(), Errno> {
                Err(14)
            }
            fn copy_to_user(&mut self, _: u64, _: &[u8]) -> std::result::Result<(), Errno> {
                Err(14)
            }
            fn get_file(&mut self, _: u32) -> std::result::Result<File, Errno> {
                Err(9)
            }
            fn install_file(&mut self, _: File) -> std::result::Result<u32, Errno> {
                Err(24)
            }
            fn close_fd(&mut self, _: u32) {
                panic!("context-manager setup must not install FDs")
            }
        }
        struct Registry {
            process: Weak<LocalProcess>,
            names: Mutex<HashMap<String, Strong>>,
        }
        impl Service for Registry {
            fn descriptor(&self) -> &str {
                "test.user.registry"
            }
            fn transact(&self, call: &mut Call<'_>) -> Reply {
                let name = call.data.read_string16()?.unwrap();
                let mut reply = Parcel::new();
                if call.code == 1 {
                    let Some(Binder::Handle(handle)) = call.data.read_binder()? else {
                        return Err(BAD_VALUE);
                    };
                    self.names
                        .lock()
                        .unwrap()
                        .insert(name, self.process.upgrade().unwrap().strong(handle));
                } else {
                    reply.write_binder(self.names.lock().unwrap().get(&name).map(Strong::binder));
                }
                Ok(reply)
            }
        }
        fn open(driver: &Arc<Driver>, pid: i32, uid: u32) -> Arc<LocalProcess> {
            LocalProcess::open(
                driver,
                Device::Binder,
                Credentials {
                    pid,
                    euid: uid,
                    security_context: None,
                },
            )
        }
        fn publish(
            server: &LocalProcess,
            client: &LocalProcess,
            name: &str,
            binder: Binder,
        ) -> Strong {
            let mut add = Parcel::new();
            add.write_string16(Some(name));
            add.write_binder(Some(binder));
            server.transact(0, 1, &add, false).unwrap();
            let mut get = Parcel::new();
            get.write_string16(Some(name));
            let reply = client.transact(0, 2, &get, false).unwrap();
            let Some(Binder::Handle(handle)) = reply.reader().read_binder().unwrap() else {
                panic!("published node")
            };
            client.strong(handle)
        }
        struct CallerOwner;
        impl Service for CallerOwner {
            fn descriptor(&self) -> &str {
                user::DESCRIPTOR
            }
            fn transact(&self, call: &mut Call<'_>) -> Reply {
                assert_eq!(call.code, user::HANDLE_CREATE_USER_COMMAND);
                assert_eq!((call.sender_pid, call.sender_euid), (190425, 2000));
                let request = user::HandleCreateUserCommand::<IncomingFd>::read(&mut call.data)?;
                assert!(
                    request.input.is_none() && request.output.is_none() && request.error.is_none()
                );
                let mut reply = Parcel::new();
                user::write_handle_create_user_command_reply(&mut reply, -23);
                Ok(reply)
            }
        }
        struct Forward {
            process: Weak<LocalProcess>,
            owner: Strong,
        }
        impl Service for Forward {
            fn descriptor(&self) -> &str {
                "test.art.forward"
            }
            fn transact(&self, call: &mut Call<'_>) -> Reply {
                let args = user::HandleCreateUserCommand::<IncomingFd>::read(&mut call.data)?;
                let mut request = Parcel::new();
                user::HandleCreateUserCommand::<Descriptor> {
                    input: None,
                    output: None,
                    error: None,
                    arguments: args.arguments,
                }
                .write(&mut request);
                let result =
                    transact_user_command(&self.process.upgrade().unwrap(), &self.owner, &request)
                        .unwrap();
                let mut reply = Parcel::new();
                user::write_handle_create_user_command_reply(&mut reply, result);
                Ok(reply)
            }
        }
        let driver = Driver::new();
        let manager = open(&driver, 190421, 1000);
        let Binder::Local(ptr) = manager.add_service(Arc::new(Registry {
            process: Arc::downgrade(&manager),
            names: Mutex::new(HashMap::new()),
        })) else {
            panic!("local registry")
        };
        let mut object = FlatBinderObject {
            kind: BINDER_TYPE_BINDER,
            flags: 0,
            binder: ptr,
            cookie: ptr,
        }
        .encode();
        driver
            .ioctl(
                manager.proc_handle(),
                190421,
                BINDER_SET_CONTEXT_MGR_EXT,
                &mut object,
                &mut NoUserMemory,
            )
            .unwrap();
        manager.start();
        let native = open(&driver, 190422, 1000);
        native.start();
        let java = open(&driver, 190423, 1000);
        java.start();
        let shell = open(&driver, 190425, 2000);
        shell.start();
        let owner = publish(
            &java,
            &native,
            "profile",
            java.add_service(Arc::new(CallerOwner)),
        );
        let mut request = Parcel::new();
        user::HandleCreateUserCommand::<Descriptor> {
            input: None,
            output: None,
            error: None,
            arguments: Some(vec![Some("create-user".into())]),
        }
        .write(&mut request);
        assert_eq!(
            transact_user_command(&native, &owner, &request)
                .unwrap_err()
                .code,
            aim_binder_host::parcel::EX_SECURITY
        );
        let ingress = native.add_service(Arc::new(Forward {
            process: Arc::downgrade(&native),
            owner,
        }));
        let ingress = publish(&native, &shell, "ingress", ingress);
        let reply = ingress
            .transact(user::HANDLE_CREATE_USER_COMMAND, &request, false)
            .unwrap();
        assert_eq!(read_user_reply(&mut reply.reader()).unwrap(), -23);
        drop(reply);
        drop(ingress);
        for process in [&shell, &native, &java, &manager] {
            driver.release(process.proc_handle());
        }
    }
}
