//! PackageInstallerSession checksum state and exact pinned Parcelable/binary codecs.
//! AOSP android-16.0.0_r1, Apache-2.0. File algorithms use the original typed helper.
use super::{Record, Session, hardlink::Files};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel, Reader,
    Result as WireResult,
};
use aim_service_aidl::{ReadParcelable, WriteParcelable, read_byte_array, write_byte_array};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checksum {
    pub kind: i32,
    pub value: Vec<u8>,
}
impl ReadParcelable for Checksum {
    fn read_from(r: &mut Reader<'_>) -> WireResult<Self> {
        Ok(Self {
            kind: r.read_i32()?,
            value: read_byte_array(r)?.ok_or(BAD_VALUE)?,
        })
    }
}
impl WriteParcelable for Checksum {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.kind);
        write_byte_array(p, Some(&self.value));
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PerFile {
    pub checksums: Vec<Checksum>,
    pub signature: Option<Vec<u8>>,
}
/// Preserve the complete tagged raw list; original certificate decoding owns type errors.
#[derive(Clone, Debug, PartialEq)]
pub struct TrustedInstallers(pub Option<Vec<aim_service_aidl::ParcelValue>>);
impl TrustedInstallers {
    pub fn read(r: &mut Reader<'_>) -> WireResult<Self> {
        Ok(Self(aim_service_aidl::read_array_list(r)?))
    }
    pub fn write(&self, p: &mut Parcel) {
        aim_service_aidl::write_array_list(p, self.0.as_deref());
    }
    pub fn certificates(&self) -> Option<Vec<u8>> {
        self.0.as_ref().map(|values| {
            let mut p = Parcel::new();
            aim_service_aidl::write_array_list(&mut p, Some(values));
            p.data().to_vec()
        })
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pending(pub Vec<(Option<String>, PerFile)>);
pub fn installer_name(record: &Record) -> Option<String> {
    if matches!(
        record.initiating_package.as_deref(),
        Some("com.android.shell") | None
    ) {
        record.installer_package.clone()
    } else {
        record.initiating_package.clone()
    }
}
impl Pending {
    /// Caller/AppOps and installer existence checks occur before signature/owner checks.
    pub fn set(
        &mut self,
        session: &Session,
        record: &Record,
        uid: u32,
        name: Option<String>,
        checksums: Vec<Option<Checksum>>,
        signature: Option<Vec<u8>>,
        checked_installer: &str,
        committed: bool,
        files: &Files,
    ) -> Result<(), Exception> {
        if checksums.is_empty() {
            return Ok(());
        }
        let package = installer_name(record)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "Installer package is empty."))?;
        if package != checked_installer {
            return Err(Exception::security("checksum installer identity changed"));
        }
        if let Some(signature) = signature.as_ref().filter(|value| !value.is_empty()) {
            files.verify(nullable_checksum_parcel(&checksums), signature.clone())?;
        }
        let checksums = checksums
            .into_iter()
            .map(|value| value.ok_or_else(|| Exception::new(EX_NULL_POINTER, "null checksum")))
            .collect::<Result<Vec<_>, _>>()?;
        if checksums.iter().any(|checksum| checksum.value.len() > 64) {
            return Err(Exception::illegal_argument("Invalid checksum."));
        }
        if uid != 0 && uid != session.installer_uid {
            return Err(Exception::security("Session does not belong to caller"));
        }
        if !session.prepared {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "addChecksums before prepared",
            ));
        }
        if session.destroyed {
            return Err(Exception::security(
                "addChecksums not allowed after destruction",
            ));
        }
        if committed {
            return Err(Exception::security("addChecksums not allowed after commit"));
        }
        if self.0.iter().any(|(key, _)| key == &name) {
            return Err(Exception::new(EX_ILLEGAL_STATE, "Duplicate checksums."));
        }
        self.insert(
            name,
            PerFile {
                checksums,
                signature,
            },
        );
        Ok(())
    }
    /// Original commit consumes the old entry when staging the renamed APK's digest files.
    pub fn take(&mut self, original_name: &str) -> Option<PerFile> {
        self.0
            .iter()
            .position(|(name, _)| name.as_deref() == Some(original_name))
            .map(|index| self.0.remove(index).1)
    }
    fn insert(&mut self, name: Option<String>, value: PerFile) {
        let hash = name
            .as_deref()
            .map(crate::package::info::java_hash)
            .unwrap_or(0);
        let index = self
            .0
            .iter()
            .position(|(key, _)| {
                key.as_deref()
                    .map(crate::package::info::java_hash)
                    .unwrap_or(0)
                    > hash
            })
            .unwrap_or(self.0.len());
        self.0.insert(index, (name, value));
    }
    pub fn read_xml(parent: &aim_android_xml::Element) -> Result<Self, String> {
        let mut result = Self::default();
        let mut signatures = Vec::new();
        for child in parent.children() {
            let name = child.string("name").map(|value| value.into_owned());
            match child.name.as_str() {
                "sessionChecksum" => {
                    let checksum = Checksum {
                        kind: child.int("checksumKind")?.unwrap_or(0),
                        value: child
                            .bytes_base64("checksumValue")?
                            .ok_or("missing checksumValue")?,
                    };
                    if let Some((_, value)) = result.0.iter_mut().find(|(key, _)| key == &name) {
                        value.checksums.push(checksum);
                    } else {
                        result.insert(
                            name,
                            PerFile {
                                checksums: vec![checksum],
                                signature: None,
                            },
                        );
                    }
                }
                "sessionChecksumSignature" => {
                    signatures.push((name, child.bytes_base64("signature")?))
                }
                _ => {}
            }
        }
        for (name, signature) in signatures {
            if let Some((_, value)) = result.0.iter_mut().find(|(key, _)| key == &name) {
                value.signature = signature;
            }
        }
        Ok(result)
    }
    pub fn append_xml(&self, parent: &mut aim_android_xml::Element) {
        use aim_android_xml::{Element, Node, Value};
        for (name, value) in &self.0 {
            for checksum in &value.checksums {
                let mut attrs = vec![
                    ("checksumKind".into(), Value::Int(checksum.kind)),
                    (
                        "checksumValue".into(),
                        Value::BytesBase64(checksum.value.clone()),
                    ),
                ];
                if let Some(name) = name {
                    attrs.insert(0, ("name".into(), Value::String(name.clone())));
                }
                parent.content.push(Node::Element(Element {
                    name: "sessionChecksum".into(),
                    attrs,
                    content: Vec::new(),
                }));
            }
        }
        for (name, value) in &self.0 {
            if let Some(signature) = value.signature.as_ref().filter(|value| !value.is_empty()) {
                let mut attrs = vec![("signature".into(), Value::BytesBase64(signature.clone()))];
                if let Some(name) = name {
                    attrs.insert(0, ("name".into(), Value::String(name.clone())));
                }
                parent.content.push(Node::Element(Element {
                    name: "sessionChecksumSignature".into(),
                    attrs,
                    content: Vec::new(),
                }));
            }
        }
    }
    /// Called after an APK's actual original and final staged names are selected.
    /// Removes the pending entry before verification, as maybeStageDigestsLocked does.
    pub fn stage_apk(
        &mut self,
        original_name: &str,
        target_name: &str,
        session: &Session,
        record: &Record,
        store: &super::storage::Store,
        files: &Files,
    ) -> Result<Vec<String>, StageError> {
        use std::io::Write;
        let Some(checksums) = self.take(original_name) else {
            return Ok(Vec::new());
        };
        if checksums.checksums.is_empty() {
            return Ok(Vec::new());
        }
        let stem = target_name
            .strip_suffix(".apk")
            .ok_or_else(|| StageError::Name(target_name.into()))?;
        checksums
            .reverify(files)
            .map_err(StageError::Verification)?;
        let digests_name = format!("{stem}.digests");
        let mut target = store
            .write_target(session, record, &digests_name, 0)
            .map_err(StageError::Storage)?;
        target
            .set_len(0)
            .and_then(|_| target.write_all(&checksums.digests()))
            .and_then(|_| target.sync_all())
            .map_err(|error| {
                StageError::Storage(super::storage::Error {
                    legacy_status: -110,
                    committed: false,
                    message: error.to_string(),
                })
            })?;
        let mut staged = vec![digests_name];
        if let Some(signature) = checksums.signature.filter(|bytes| !bytes.is_empty()) {
            let signature_name = format!("{stem}.digests.signature");
            let mut target = store
                .write_target(session, record, &signature_name, 0)
                .map_err(StageError::Storage)?;
            target
                .set_len(0)
                .and_then(|_| target.write_all(&signature))
                .and_then(|_| target.sync_all())
                .map_err(|error| {
                    StageError::Storage(super::storage::Error {
                    legacy_status: -110,
                        committed: false,
                        message: error.to_string(),
                    })
                })?;
            staged.push(signature_name);
        }
        Ok(staged)
    }
    pub fn require_consumed(&self) -> Result<(), StageError> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(StageError::Remaining(
                self.0.iter().map(|(name, _)| name.clone()).collect(),
            ))
        }
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
#[derive(Debug)]
pub enum StageError {
    Verification(Exception),
    Storage(super::storage::Error),
    Name(String),
    Remaining(Vec<Option<String>>),
}
fn nullable_checksum_parcel(checksums: &[Option<Checksum>]) -> Vec<u8> {
    let mut p = Parcel::new();
    p.write_i32(checksums.len() as i32);
    for value in checksums {
        aim_service_aidl::write_typed(&mut p, value.as_ref());
    }
    p.data().to_vec()
}
pub fn checksum_parcel(checksums: &[Checksum]) -> Vec<u8> {
    let mut p = Parcel::new();
    p.write_i32(checksums.len() as i32);
    for checksum in checksums {
        p.write_i32(1);
        checksum.write_to(&mut p);
    }
    p.data().to_vec()
}
impl PerFile {
    /// Original Checksum.writeToStream: big-endian int kind, int length, raw bytes.
    pub fn digests(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        for checksum in &self.checksums {
            bytes.extend_from_slice(&checksum.kind.to_be_bytes());
            bytes.extend_from_slice(&(checksum.value.len() as i32).to_be_bytes());
            bytes.extend_from_slice(&checksum.value);
        }
        bytes
    }
    pub fn reverify(&self, files: &Files) -> Result<(), Exception> {
        if let Some(signature) = self.signature.as_ref().filter(|bytes| !bytes.is_empty()) {
            files.verify(checksum_parcel(&self.checksums), signature.clone())?;
        }
        Ok(())
    }
}
pub struct Requests {
    pub files: Arc<Files>,
}
impl Requests {
    pub fn request(
        &self,
        session: &Session,
        record: &Record,
        uid: u32,
        verifier: bool,
        guest_path: String,
        optional: i32,
        required: i32,
        trusted: TrustedInstallers,
        listener: Option<Binder>,
    ) -> Result<(), Exception> {
        if uid != 0 && uid != session.installer_uid && !verifier {
            return Err(Exception::security("Session does not belong to caller"));
        }
        self.files.request(
            guest_path,
            installer_name(record),
            optional,
            required,
            trusted.certificates(),
            listener,
        )
    }
}
