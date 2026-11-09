//! Vold's Keystore (android-16.0.0_r1, system/vold/Keystore.cpp): keystore2's
//! TRUSTED_ENVIRONMENT security level for the keys that seal storage keys,
//! of `Domain::BLOB` in vold's namespace, and earlyBootEnded. Keystore owns
//! early-boot key retirement and the boot-level key cache, including on
//! devices without metadata encryption or checkpoints.

use std::io;

use aim_framework_aidl::{
    android_security_maintenance_ikeystoremaintenance as maintenance,
    android_system_keystore2_ikeystoreoperation as operation,
    android_system_keystore2_ikeystoresecuritylevel as security_level,
    android_system_keystore2_ikeystoreservice as service,
};
use binder::binder_impl::{
    BorrowedParcel, FLAG_CLEAR_BUF, IBinderInternal, NON_NULL_PARCELABLE_FLAG, Parcel,
    TransactionCode,
};
use binder::{
    FromIBinder, Interface, Parcelable, SpIBinder, Status, StatusCode, Strong,
    declare_binder_interface, impl_deserialize_for_parcelable, impl_serialize_for_parcelable,
};
use vold::key_storage::{GCM_MAC_BYTES, GCM_NONCE_BYTES, KeyBuffer, Keystore, Output};

const KEYSTORE2: &str = "android.system.keystore2.IKeystoreService/default";
/// system/sepolicy's keystore2_key_contexts.
const VOLD_NAMESPACE: i64 = 100;
const DOMAIN_BLOB: i32 = 3;
const SECURITY_LEVEL_TRUSTED_ENVIRONMENT: i32 = 1;
/// IKeystoreOperation's update() limit.
const UPDATE_INPUT_MAX_SIZE: usize = 32 * 1024;

/// KeyMint's Tag (TagType | number) and its values.
const TAG_PURPOSE: i32 = 0x2000_0001;
const TAG_ALGORITHM: i32 = 0x1000_0002;
const TAG_KEY_SIZE: i32 = 0x3000_0003;
const TAG_BLOCK_MODE: i32 = 0x2000_0004;
const TAG_PADDING: i32 = 0x2000_0006;
const TAG_MIN_MAC_LENGTH: i32 = 0x3000_0008;
const TAG_ROLLBACK_RESISTANCE: i32 = 0x7000_012f;
const TAG_NO_AUTH_REQUIRED: i32 = 0x7000_01f7;
const TAG_APPLICATION_ID: i32 = 0x9000_0259_u32 as i32;
const TAG_NONCE: i32 = 0x9000_03e9_u32 as i32;
const TAG_MAC_LENGTH: i32 = 0x3000_03eb;
const ALGORITHM_AES: i32 = 32;
const BLOCK_MODE_GCM: i32 = 32;
const PADDING_NONE: i32 = 1;
const PURPOSE_ENCRYPT: i32 = 0;
const PURPOSE_DECRYPT: i32 = 1;

fn unused<T: ?Sized>(
    _: &T,
    _: TransactionCode,
    _: &BorrowedParcel<'_>,
    _: &mut BorrowedParcel<'_>,
) -> Result<(), StatusCode> {
    Err(StatusCode::UNKNOWN_TRANSACTION)
}

macro_rules! client {
    ($name:ident, $descriptor:expr, $native:ident, $proxy:ident) => {
        trait $name: Interface {}
        declare_binder_interface! {
            $name[$descriptor] {
                native: $native(unused),
                proxy: $proxy,
            }
        }
        impl $name for $proxy {}
        impl $name for binder::binder_impl::Binder<$native> {}
    };
}

client!(
    IKeystoreMaintenance,
    maintenance::DESCRIPTOR,
    BnKeystoreMaintenance,
    BpKeystoreMaintenance
);
client!(
    IKeystoreService,
    service::DESCRIPTOR,
    BnKeystoreService,
    BpKeystoreService
);
client!(
    IKeystoreSecurityLevel,
    security_level::DESCRIPTOR,
    BnSecurityLevel,
    BpSecurityLevel
);
client!(
    IKeystoreOperation,
    operation::DESCRIPTOR,
    BnOperation,
    BpOperation
);

/// keystore2's KeyDescriptor.
#[derive(Default)]
struct KeyDescriptor {
    domain: i32,
    nspace: i64,
    alias: Option<String>,
    blob: Option<Vec<u8>>,
}

impl KeyDescriptor {
    fn blob(blob: Option<&[u8]>) -> Self {
        KeyDescriptor {
            domain: DOMAIN_BLOB,
            nspace: VOLD_NAMESPACE,
            alias: None,
            blob: blob.map(<[u8]>::to_vec),
        }
    }
}

impl Parcelable for KeyDescriptor {
    fn write_to_parcel(&self, parcel: &mut BorrowedParcel<'_>) -> Result<(), StatusCode> {
        parcel.sized_write(|p| {
            p.write(&self.domain)?;
            p.write(&self.nspace)?;
            p.write(&self.alias)?;
            p.write(&self.blob)
        })
    }
    fn read_from_parcel(&mut self, parcel: &BorrowedParcel<'_>) -> Result<(), StatusCode> {
        parcel.sized_read(|p| {
            self.domain = p.read()?;
            self.nspace = p.read()?;
            self.alias = p.read()?;
            self.blob = p.read()?;
            Ok(())
        })
    }
}
impl_serialize_for_parcelable!(KeyDescriptor);
impl_deserialize_for_parcelable!(KeyDescriptor);

/// KeyMint's KeyParameter; its KeyParameterValue union holds an int-backed
/// value (fields 0-11), a long (12-13) or a blob (14).
#[derive(Default)]
struct KeyParameter {
    tag: i32,
    field: i32,
    int: i32,
    long: i64,
    blob: Vec<u8>,
}

impl KeyParameter {
    /// A value of the union field `field` (algorithm 1, blockMode 2,
    /// paddingMode 3, keyPurpose 7, boolValue 10, integer 11).
    fn int(tag: i32, field: i32, int: i32) -> Self {
        KeyParameter {
            tag,
            field,
            int,
            ..Default::default()
        }
    }
    fn blob(tag: i32, blob: &[u8]) -> Self {
        KeyParameter {
            tag,
            field: 14,
            blob: blob.to_vec(),
            ..Default::default()
        }
    }
}

impl Parcelable for KeyParameter {
    fn write_to_parcel(&self, parcel: &mut BorrowedParcel<'_>) -> Result<(), StatusCode> {
        parcel.sized_write(|p| {
            p.write(&self.tag)?;
            p.write(&NON_NULL_PARCELABLE_FLAG)?;
            p.write(&self.field)?;
            match self.field {
                0..=11 => p.write(&self.int),
                12 | 13 => p.write(&self.long),
                _ => p.write(&self.blob),
            }
        })
    }
    fn read_from_parcel(&mut self, parcel: &BorrowedParcel<'_>) -> Result<(), StatusCode> {
        parcel.sized_read(|p| {
            self.tag = p.read()?;
            if p.read::<i32>()? != NON_NULL_PARCELABLE_FLAG {
                return Err(StatusCode::UNEXPECTED_NULL);
            }
            self.field = p.read()?;
            match self.field {
                0..=11 => self.int = p.read()?,
                12 | 13 => self.long = p.read()?,
                14 => self.blob = p.read()?,
                _ => return Err(StatusCode::BAD_VALUE),
            }
            Ok(())
        })
    }
}
impl_serialize_for_parcelable!(KeyParameter);
impl_deserialize_for_parcelable!(KeyParameter);

/// keystore2's KeyParameters and OperationChallenge, as a
/// CreateOperationResponse carries them.
#[derive(Default)]
struct KeyParameters(Vec<KeyParameter>);

impl Parcelable for KeyParameters {
    fn write_to_parcel(&self, parcel: &mut BorrowedParcel<'_>) -> Result<(), StatusCode> {
        parcel.sized_write(|p| p.write(&self.0))
    }
    fn read_from_parcel(&mut self, parcel: &BorrowedParcel<'_>) -> Result<(), StatusCode> {
        parcel.sized_read(|p| {
            self.0 = p.read()?;
            Ok(())
        })
    }
}
impl_serialize_for_parcelable!(KeyParameters);
impl_deserialize_for_parcelable!(KeyParameters);

#[derive(Default)]
struct OperationChallenge(i64);

impl Parcelable for OperationChallenge {
    fn write_to_parcel(&self, parcel: &mut BorrowedParcel<'_>) -> Result<(), StatusCode> {
        parcel.sized_write(|p| p.write(&self.0))
    }
    fn read_from_parcel(&mut self, parcel: &BorrowedParcel<'_>) -> Result<(), StatusCode> {
        parcel.sized_read(|p| {
            self.0 = p.read()?;
            Ok(())
        })
    }
}
impl_serialize_for_parcelable!(OperationChallenge);
impl_deserialize_for_parcelable!(OperationChallenge);

fn io_error(what: &str, e: impl std::fmt::Debug) -> io::Error {
    log::error!("keystore2 {what}: {e:?}");
    io::Error::other(format!("keystore2 {what}: {e:?}"))
}

/// A transaction of keystore2's sensitive interfaces: the reply after its
/// status.
fn call(
    binder: &SpIBinder,
    code: u32,
    write: impl FnOnce(&mut BorrowedParcel<'_>) -> Result<(), StatusCode>,
) -> binder::Result<Parcel> {
    let mut data = binder.prepare_transact()?;
    data.mark_sensitive();
    write(&mut data.borrowed())?;
    let reply = binder.submit_transact(code, data, FLAG_CLEAR_BUF)?;
    let status: Status = reply.read()?;
    if status.is_ok() {
        Ok(reply)
    } else {
        Err(status)
    }
}

pub fn early_boot_ended() -> binder::Result<()> {
    let service =
        binder::wait_for_interface::<dyn IKeystoreMaintenance>("android.security.maintenance")?;
    call(&service.as_binder(), maintenance::EARLY_BOOT_ENDED, |_| {
        Ok(())
    })
    .map(drop)
}

/// Read the non-null flag of a parcelable a reply holds.
fn non_null(reply: &Parcel) -> binder::Result<()> {
    if reply.read::<i32>()? != NON_NULL_PARCELABLE_FLAG {
        return Err(StatusCode::UNEXPECTED_NULL.into());
    }
    Ok(())
}

/// keystore2 for vold's storage keys, connected anew for each use as
/// vold's Keystore is constructed for each.
pub struct Keystore2;

/// Keystore::Keystore: TRUSTED_ENVIRONMENT, which keystore2 serves with its
/// software KeyMint when there is no TEE.
fn security_level() -> io::Result<SpIBinder> {
    (|| {
        let keystore = binder::wait_for_interface::<dyn IKeystoreService>(KEYSTORE2)?;
        let reply = call(&keystore.as_binder(), service::GET_SECURITY_LEVEL, |p| {
            p.write(&SECURITY_LEVEL_TRUSTED_ENVIRONMENT)
        })?;
        let level: Strong<dyn IKeystoreSecurityLevel> = FromIBinder::try_from(reply.read()?)?;
        Ok::<_, Status>(level.as_binder())
    })()
    .map_err(|e| io_error("getSecurityLevel", e))
}

impl Keystore2 {
    fn generate(&self, params: &[KeyParameter]) -> io::Result<Vec<u8>> {
        let level = security_level()?;
        let mut key = KeyDescriptor::default();
        (|| {
            let reply = call(&level, security_level::GENERATE_KEY, |p| {
                p.write(&KeyDescriptor::blob(None))?;
                p.write(&None::<KeyDescriptor>)?;
                p.write(params)?;
                p.write(&0i32)?;
                p.write(&Vec::<u8>::new())
            })?;
            // KeyMetadata: its key comes first.
            non_null(&reply)?;
            reply.sized_read(|p| {
                key = p.read()?;
                Ok(())
            })?;
            Ok::<_, Status>(())
        })()
        .map_err(|e| io_error("generateKey", e))?;
        key.blob
            .ok_or_else(|| io_error("generateKey", "generated key blob was null"))
    }

    /// Keystore::begin, then updateCompletely and finish: the output, with
    /// the upgraded blob, and the nonce Keystore chose, if any.
    fn operate(
        &self,
        blob: &[u8],
        params: &[KeyParameter],
        input: &[u8],
    ) -> io::Result<(Output, Option<Vec<u8>>)> {
        let level = security_level()?;
        let mut op: Option<SpIBinder> = None;
        let mut out = None::<KeyParameters>;
        let mut upgraded = None;
        let op: Strong<dyn IKeystoreOperation> = (|| {
            let reply = call(&level, security_level::CREATE_OPERATION, |p| {
                p.write(&KeyDescriptor::blob(Some(blob)))?;
                p.write(params)?;
                p.write(&true)
            })?;
            non_null(&reply)?;
            reply.sized_read(|p| {
                op = p.read()?;
                let _: Option<OperationChallenge> = p.read()?;
                out = p.read()?;
                upgraded = p.read()?;
                Ok(())
            })?;
            Ok::<_, Status>(FromIBinder::try_from(
                op.take().ok_or(StatusCode::UNEXPECTED_NULL)?,
            )?)
        })()
        .map_err(|e| io_error("createOperation", e))?;
        let op = op.as_binder();
        let mut data = KeyBuffer(Vec::new());
        let result = (|| {
            for chunk in input.chunks(UPDATE_INPUT_MAX_SIZE) {
                let reply = call(&op, operation::UPDATE, |p| p.write(chunk))?;
                let output: Option<Vec<u8>> = reply.read()?;
                data.0.extend(output.unwrap_or_default());
            }
            let reply = call(&op, operation::FINISH, |p| {
                p.write(&None::<Vec<u8>>)?;
                p.write(&None::<Vec<u8>>)
            })?;
            let output: Option<Vec<u8>> = reply.read()?;
            data.0.extend(output.unwrap_or_default());
            Ok::<_, Status>(())
        })();
        if let Err(e) = result {
            // ~KeystoreOperation aborts an unfinished operation.
            let _ = call(&op, operation::ABORT, |_| Ok(()));
            return Err(io_error("update/finish", e));
        }
        let nonce = out
            .into_iter()
            .flat_map(|o| o.0)
            .find(|p| p.tag == TAG_NONCE)
            .map(|p| p.blob);
        Ok((Output { data, upgraded }, nonce))
    }
}

/// beginParams, then the operation's own parameters.
fn op_params(app_id: &[u8], nonce: Option<&[u8]>, purpose: i32) -> Vec<KeyParameter> {
    let mut params = vec![
        KeyParameter::int(TAG_BLOCK_MODE, 2, BLOCK_MODE_GCM),
        KeyParameter::int(TAG_PADDING, 3, PADDING_NONE),
        KeyParameter::int(TAG_MAC_LENGTH, 11, (GCM_MAC_BYTES * 8) as i32),
        KeyParameter::blob(TAG_APPLICATION_ID, app_id),
    ];
    params.extend(nonce.map(|n| KeyParameter::blob(TAG_NONCE, n)));
    params.push(KeyParameter::int(TAG_PURPOSE, 7, purpose));
    params
}

impl Keystore for Keystore2 {
    fn generate_key(&self, app_id: &[u8]) -> io::Result<Vec<u8>> {
        let mut params = vec![
            KeyParameter::int(TAG_ALGORITHM, 1, ALGORITHM_AES),
            KeyParameter::int(TAG_KEY_SIZE, 11, 256),
            KeyParameter::int(TAG_PURPOSE, 7, PURPOSE_ENCRYPT),
            KeyParameter::int(TAG_PURPOSE, 7, PURPOSE_DECRYPT),
            KeyParameter::int(TAG_BLOCK_MODE, 2, BLOCK_MODE_GCM),
            KeyParameter::int(TAG_PADDING, 3, PADDING_NONE),
            KeyParameter::int(TAG_MIN_MAC_LENGTH, 11, (GCM_MAC_BYTES * 8) as i32),
            KeyParameter::blob(TAG_APPLICATION_ID, app_id),
            KeyParameter::int(TAG_NO_AUTH_REQUIRED, 10, 1),
        ];
        let without_rollback = params.len();
        params.push(KeyParameter::int(TAG_ROLLBACK_RESISTANCE, 10, 1));
        self.generate(&params).or_else(|_| {
            log::warn!(
                "Failed to generate rollback-resistant key; falling back to a non-rollback-resistant one"
            );
            self.generate(&params[..without_rollback])
        })
    }

    fn encrypt(&self, blob: &[u8], app_id: &[u8], message: &[u8]) -> io::Result<Output> {
        let (body, nonce) =
            self.operate(blob, &op_params(app_id, None, PURPOSE_ENCRYPT), message)?;
        let nonce =
            nonce.ok_or_else(|| io_error("encrypt", "GCM encryption but no nonce generated"))?;
        if nonce.len() != GCM_NONCE_BYTES {
            return Err(io_error("encrypt", "wrong nonce size"));
        }
        Ok(Output {
            data: KeyBuffer([nonce.as_slice(), &body.data.0].concat()),
            upgraded: body.upgraded,
        })
    }

    fn decrypt(&self, blob: &[u8], app_id: &[u8], sealed: &[u8]) -> io::Result<Output> {
        if sealed.len() < GCM_NONCE_BYTES {
            return Err(io_error("decrypt", "ciphertext too small"));
        }
        let (nonce, body) = sealed.split_at(GCM_NONCE_BYTES);
        let params = op_params(app_id, Some(nonce), PURPOSE_DECRYPT);
        Ok(self.operate(blob, &params, body)?.0)
    }

    fn delete_key(&self, blob: &[u8]) -> io::Result<()> {
        call(&security_level()?, security_level::DELETE_KEY, |p| {
            p.write(&KeyDescriptor::blob(Some(blob)))
        })
        .map(drop)
        .map_err(|e| io_error("deleteKey", e))
    }
}
