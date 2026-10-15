//! Original guest VFS/SELinux leaves. Native host paths never cross this boundary.
use aim_binder_host::{
    local::Strong,
    parcel::{Binder, EX_ILLEGAL_STATE, Exception, Parcel},
};
use aim_service_aidl::dev_aim_server_ipackageinstallerfiles as aidl;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub struct Files {
    bridge: Strong,
    closed: AtomicBool,
}
impl Files {
    pub fn new(bridge: Strong) -> Arc<Self> {
        Arc::new(Self {
            bridge,
            closed: AtomicBool::new(false),
        })
    }
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
    fn current(&self) -> Result<(), Exception> {
        if self.closed.load(Ordering::SeqCst) {
            Err(Exception::new(
                EX_ILLEGAL_STATE,
                "installer file bridge closed",
            ))
        } else {
            Ok(())
        }
    }
    fn call(
        &self,
        code: u32,
        parcel: &Parcel,
    ) -> Result<aim_binder_host::local::Received, Exception> {
        self.current()?;
        let reply = self
            .bridge
            .transact(code, parcel, false)
            .map_err(|status| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    format!("installer file bridge transport: {status}"),
                )
            })?;
        self.current()?;
        Ok(reply)
    }
    /// Pinned session method checks exactly SYSTEM_UID, including rejecting root.
    pub fn stage(
        &self,
        uid: u32,
        source: Option<String>,
        stage_directory: String,
    ) -> Result<(), Exception> {
        if uid != 1000 {
            return Err(Exception::security("link() can only be run by the system"));
        }
        let mut p = Parcel::new();
        aidl::StageViaHardLink {
            source,
            stage_directory: Some(stage_directory),
        }
        .write(&mut p);
        let reply = self.call(aidl::STAGE_VIA_HARD_LINK, &p)?;
        aidl::read_stage_via_hard_link_reply(&mut reply.reader()).map_err(|status| {
            Exception::new(
                EX_ILLEGAL_STATE,
                format!("installer hardlink reply: {status}"),
            )
        })?
    }
    pub fn verify(
        &self,
        checksums: Vec<u8>,
        signature: Vec<u8>,
    ) -> Result<Vec<Vec<u8>>, Exception> {
        let mut p = Parcel::new();
        aidl::VerifyChecksums {
            checksum_parcel: Some(checksums),
            signature: Some(signature),
        }
        .write(&mut p);
        let reply = self.call(aidl::VERIFY_CHECKSUMS, &p)?;
        let bytes = aidl::read_verify_checksums_reply(&mut reply.reader())
            .map_err(|status| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    format!("checksum verification reply: {status}"),
                )
            })??
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "missing verified certificates"))?;
        let mut r = aim_binder_host::parcel::Reader::new(&bytes, &[]);
        let bad = || Exception::new(EX_ILLEGAL_STATE, "invalid verified certificate record");
        let count: usize = r
            .read_i32()
            .map_err(|_| bad())?
            .try_into()
            .map_err(|_| bad())?;
        if count > bytes.len() / 4 {
            return Err(bad());
        }
        let certs = (0..count)
            .map(|_| {
                aim_service_aidl::read_byte_array(&mut r)
                    .map_err(|_| bad())?
                    .ok_or_else(bad)
            })
            .collect::<Result<Vec<_>, _>>()?;
        if r.remaining() != 0 || certs.is_empty() {
            return Err(bad());
        }
        Ok(certs)
    }
    pub fn request_package(
        &self,
        files: &[(Option<String>, String)],
        installer_package: Option<String>,
        optional: i32,
        required: i32,
        trusted: Option<Vec<u8>>,
        listener: Option<Binder>,
    ) -> Result<(), Exception> {
        let mut body = Parcel::new();
        body.write_i32(files.len() as i32);
        for (split, path) in files {
            body.write_string16(split.as_deref());
            body.write_string16(Some(path));
        }
        let mut p = Parcel::new();
        aidl::RequestPackageChecksums {
            file_parcel: Some(body.data().to_vec()),
            installer_package,
            optional,
            required,
            trusted_certificate_parcel: trusted,
            listener,
        }
        .write(&mut p);
        let reply = self.call(aidl::REQUEST_PACKAGE_CHECKSUMS, &p)?;
        aidl::read_request_package_checksums_reply(&mut reply.reader()).map_err(|status| {
            Exception::new(
                EX_ILLEGAL_STATE,
                format!("package checksum reply: {status}"),
            )
        })?
    }
    pub fn request(
        &self,
        guest_path: String,
        installer_package: Option<String>,
        optional: i32,
        required: i32,
        trusted: Option<Vec<u8>>,
        listener: Option<Binder>,
    ) -> Result<(), Exception> {
        let mut p = Parcel::new();
        aidl::RequestChecksums {
            guest_path: Some(guest_path),
            installer_package,
            optional,
            required,
            trusted_certificate_parcel: trusted,
            listener,
        }
        .write(&mut p);
        let reply = self.call(aidl::REQUEST_CHECKSUMS, &p)?;
        aidl::read_request_checksums_reply(&mut reply.reader()).map_err(|status| {
            Exception::new(
                EX_ILLEGAL_STATE,
                format!("checksum request reply: {status}"),
            )
        })?
    }
}
