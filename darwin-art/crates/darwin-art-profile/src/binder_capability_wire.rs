//! Fixed bounded codec. Binding bytes never authenticate a session or delivery.
use crate::ProfileError;
use darwin_art_engine_sys::{ScmBinderBindingV2, ScmGrantV2};
use darwin_art_scm_transfer::capabilities::{Binding, ClaimedDelivery};

pub(crate) enum Request {
    Bind {
        binding: Binding,
        holder: u128,
    },
    Cancel {
        binding: Binding,
    },
    Claim {
        binding: Binding,
        attributes: [u8; 40],
    },
}
fn invalid() -> ProfileError {
    ProfileError::Daemon("invalid Binder capability wire frame".into())
}
pub(crate) fn binding(raw: ScmBinderBindingV2) -> Result<Binding, ProfileError> {
    if raw.source_connection == 0 || raw.transfer == 0 {
        return Err(invalid());
    }
    Ok(Binding::Binder {
        source_connection: raw.source_connection,
        transfer: raw.transfer,
        ordinal: raw.ordinal,
        object_offset: raw.object_offset,
    })
}
fn raw(binding: Binding) -> Result<ScmBinderBindingV2, ProfileError> {
    let Binding::Binder {
        source_connection,
        transfer,
        ordinal,
        object_offset,
    } = binding
    else {
        return Err(invalid());
    };
    let raw = ScmBinderBindingV2 {
        source_connection,
        transfer,
        ordinal,
        object_offset,
    };
    self::binding(raw)?;
    Ok(raw)
}
pub(crate) fn encode(request: &Request) -> Result<Vec<u8>, ProfileError> {
    let (operation, binding) = match request {
        Request::Bind { binding, holder } => {
            if *holder == 0 {
                return Err(invalid());
            }
            (1, *binding)
        }
        Request::Cancel { binding } => (2, *binding),
        Request::Claim { binding, .. } => (3, *binding),
    };
    let raw = raw(binding)?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(76).map_err(|_| invalid())?;
    bytes.extend_from_slice(&[1, operation, 0, 0]);
    for value in [
        raw.source_connection,
        raw.transfer,
        raw.ordinal,
        raw.object_offset,
    ] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    match request {
        Request::Bind { holder, .. } => bytes.extend_from_slice(&holder.to_le_bytes()),
        Request::Claim { attributes, .. } => bytes.extend_from_slice(attributes),
        _ => {}
    }
    Ok(bytes)
}
pub(crate) fn decode(bytes: &[u8]) -> Result<Request, ProfileError> {
    if bytes.len() < 36 || bytes[0] != 1 || bytes[2..4] != [0, 0] {
        return Err(invalid());
    }
    let binding = binding(ScmBinderBindingV2 {
        source_connection: u64::from_le_bytes(bytes[4..12].try_into().unwrap()),
        transfer: u64::from_le_bytes(bytes[12..20].try_into().unwrap()),
        ordinal: u64::from_le_bytes(bytes[20..28].try_into().unwrap()),
        object_offset: u64::from_le_bytes(bytes[28..36].try_into().unwrap()),
    })?;
    match (bytes[1], bytes.len()) {
        (1, 52) => {
            let holder = u128::from_le_bytes(bytes[36..52].try_into().unwrap());
            if holder == 0 {
                return Err(invalid());
            }
            Ok(Request::Bind { binding, holder })
        }
        (2, 36) => Ok(Request::Cancel { binding }),
        (3, 76) => Ok(Request::Claim {
            binding,
            attributes: bytes[36..76].try_into().unwrap(),
        }),
        _ => Err(invalid()),
    }
}
pub(crate) fn encode_claim(claim: ClaimedDelivery) -> [u8; 41] {
    let mut bytes = [0; 41];
    let endpoint = claim.endpoint;
    bytes[..16].copy_from_slice(&endpoint.carrier.authority.instance.to_le_bytes());
    bytes[16..24].copy_from_slice(&endpoint.carrier.serial.to_le_bytes());
    bytes[24..40].copy_from_slice(&claim.grant.id().to_le_bytes());
    bytes[40] = match endpoint.side {
        darwin_art_scm_transfer::Side::A => 0,
        darwin_art_scm_transfer::Side::B => 1,
    };
    bytes
}
pub(crate) fn decode_claim(bytes: &[u8]) -> Result<ScmGrantV2, ProfileError> {
    if bytes.len() != 41 {
        return Err(invalid());
    }
    let authority = bytes[..16].try_into().unwrap();
    let holder = bytes[24..40].try_into().unwrap();
    let carrier = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
    let side = bytes[40] as u32;
    if authority == [0; 16] || holder == [0; 16] || carrier == 0 || side > 1 {
        return Err(invalid());
    }
    Ok(ScmGrantV2 {
        authority,
        carrier,
        holder,
        side,
        reserved: 0,
    })
}
