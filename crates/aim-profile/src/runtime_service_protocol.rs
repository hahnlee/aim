//! Typed wire contracts for the profile-owned runtime service.
//!
//! Startup uses payload version `2`: accepting startup requires support for
//! authenticated capability loss, not just initial readiness. Older daemons
//! reject it before spawning rather than silently losing failure reports.
//! Ready/loss/response payloads retain version `1`. Integer fields are
//! little-endian.  Lengths are `u32`, sequence counts are `u16`, and the
//! complete payload is limited to 64 KiB.  A start request is encoded as:
//!
//! ```text
//! version | key[32] | readiness_mask[u32] | package[bytes]
//!         | argv_count[u16] | argv* | env_count[u16] | env*
//! ```
//!
//! where each variable byte string is prefixed by a `u32` byte length and an
//! environment entry consists of a length-prefixed key followed by a
//! length-prefixed value.  Ready requests and start responses contain only
//! their fixed-size fields after the version byte.  A loss request uses the
//! same fixed-size shape as a ready request: `version | token[16] |
//! lost_mask[u32]`.

use crate::{ProfileError, registry::validate_package};
use std::ffi::OsString;
use std::fmt;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::str::FromStr;

const VERSION: u8 = 1;
const START_VERSION: u8 = 2;
const MAX_PAYLOAD: usize = 64 * 1024;
const MAX_ARGC: usize = 64;
const MAX_ENVC: usize = 256;
const MAX_PID: u32 = i32::MAX as u32;

fn invalid(detail: &'static str) -> ProfileError {
    ProfileError::Daemon(format!("invalid runtime service protocol: {detail}"))
}

/// The generation/image identity used to associate a runtime service with a
/// particular immutable runtime image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeKey(pub [u8; 32]);

impl RuntimeKey {
    /// Parse exactly 64 ASCII hexadecimal characters.
    ///
    /// Hexadecimal input must use one case consistently: all lowercase or all
    /// uppercase (digits are, of course, case-neutral).
    pub fn from_hex(value: &str) -> Result<Self, ProfileError> {
        if value.len() != 64 || !value.is_ascii() {
            return Err(invalid("runtime key must be 64 ASCII hex characters"));
        }
        let bytes = value.as_bytes();
        let has_lower = bytes.iter().any(u8::is_ascii_lowercase);
        let has_upper = bytes.iter().any(u8::is_ascii_uppercase);
        if has_lower && has_upper {
            return Err(invalid("runtime key hex case must be uniform"));
        }
        let mut key = [0_u8; 32];
        for (index, pair) in bytes.chunks_exact(2).enumerate() {
            let high =
                hex_digit(pair[0]).ok_or_else(|| invalid("runtime key is not hexadecimal"))?;
            let low =
                hex_digit(pair[1]).ok_or_else(|| invalid("runtime key is not hexadecimal"))?;
            key[index] = (high << 4) | low;
        }
        Ok(Self(key))
    }

    pub fn to_hex(self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut output = String::with_capacity(64);
        for byte in self.0 {
            output.push(HEX[(byte >> 4) as usize] as char);
            output.push(HEX[(byte & 0x0f) as usize] as char);
        }
        output
    }
}

impl FromStr for RuntimeKey {
    type Err = ProfileError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::from_hex(value)
    }
}

impl fmt::Display for RuntimeKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// An opaque per-start lease token owned by the profile daemon.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InstanceToken(pub [u8; 16]);

/// The service readiness capabilities requested or published by a runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadinessMask(u32);

impl ReadinessMask {
    pub const BINDER: Self = Self(1);
    pub const COMPOSITOR: Self = Self(2);
    pub const KNOWN_BITS: u32 = Self::BINDER.0 | Self::COMPOSITOR.0;

    pub fn new(bits: u32) -> Result<Self, ProfileError> {
        if bits == 0 || bits & !Self::KNOWN_BITS != 0 {
            return Err(invalid("readiness mask is empty or has unknown bits"));
        }
        Ok(Self(bits))
    }

    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn contains(self, required: Self) -> bool {
        self.0 & required.0 == required.0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// Request to start (or reserve) one runtime service instance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartRuntimeRequest {
    pub package: String,
    pub key: RuntimeKey,
    pub required: ReadinessMask,
    pub arguments: Vec<OsString>,
    pub environment: Vec<(OsString, OsString)>,
}

impl StartRuntimeRequest {
    pub fn encode(&self) -> Result<Vec<u8>, ProfileError> {
        self.validate()?;
        let mut writer = Writer::with_capacity(self.wire_len()?);
        writer.u8(START_VERSION);
        writer.fixed(&self.key.0);
        writer.u32(self.required.bits());
        writer.bytes(self.package.as_bytes());
        writer.u16(self.arguments.len() as u16);
        for argument in &self.arguments {
            writer.bytes(argument.as_os_str().as_bytes());
        }
        writer.u16(self.environment.len() as u16);
        for (name, value) in &self.environment {
            writer.bytes(name.as_os_str().as_bytes());
            writer.bytes(value.as_os_str().as_bytes());
        }
        writer.finish()
    }

    pub fn decode(payload: &[u8]) -> Result<Self, ProfileError> {
        let mut reader = Reader::new(payload)?;
        if reader.u8()? != START_VERSION {
            return Err(invalid(
                "startup requires the capability-loss-aware protocol",
            ));
        }
        let mut key = [0_u8; 32];
        reader.fixed(&mut key)?;
        let required = ReadinessMask::new(reader.u32()?)?;
        let package = reader.string()?;
        let argc = reader.u16()? as usize;
        if !(1..=MAX_ARGC).contains(&argc) {
            return Err(invalid("argument count must be 1..=64"));
        }
        let mut arguments = Vec::with_capacity(argc);
        for _ in 0..argc {
            arguments.push(reader.os_string()?);
        }
        let envc = reader.u16()? as usize;
        if envc > MAX_ENVC {
            return Err(invalid("environment count exceeds 256"));
        }
        let mut environment = Vec::with_capacity(envc);
        for _ in 0..envc {
            environment.push((reader.os_string()?, reader.os_string()?));
        }
        reader.finish()?;
        let request = Self {
            package,
            key: RuntimeKey(key),
            required,
            arguments,
            environment,
        };
        request.validate()?;
        Ok(request)
    }

    fn validate(&self) -> Result<(), ProfileError> {
        validate_package(&self.package)?;
        ReadinessMask::new(self.required.bits())?;
        if !(1..=MAX_ARGC).contains(&self.arguments.len()) {
            return Err(invalid("argument count must be 1..=64"));
        }
        if self.environment.len() > MAX_ENVC {
            return Err(invalid("environment count exceeds 256"));
        }
        for argument in &self.arguments {
            reject_nul(argument.as_os_str().as_bytes(), "argument")?;
        }
        for (name, value) in &self.environment {
            let name = name.as_os_str().as_bytes();
            reject_nul(name, "environment key")?;
            if name.is_empty() || name.contains(&b'=') {
                return Err(invalid(
                    "environment key must be nonempty and contain no '='",
                ));
            }
            reject_nul(value.as_os_str().as_bytes(), "environment value")?;
        }
        for (index, (name, _)) in self.environment.iter().enumerate() {
            if self.environment[..index]
                .iter()
                .any(|(previous, _)| previous.as_os_str().as_bytes() == name.as_os_str().as_bytes())
            {
                return Err(invalid("duplicate environment key"));
            }
        }
        Ok(())
    }

    fn wire_len(&self) -> Result<usize, ProfileError> {
        let mut length = 1_usize + self.key.0.len() + 4;
        length = checked_wire_add(length, 4)?;
        length = checked_wire_add(length, self.package.len())?;
        length = checked_wire_add(length, 2)?;
        for argument in &self.arguments {
            length = checked_wire_add(length, 4)?;
            length = checked_wire_add(length, argument.as_os_str().as_bytes().len())?;
        }
        length = checked_wire_add(length, 2)?;
        for (name, value) in &self.environment {
            length = checked_wire_add(length, 4)?;
            length = checked_wire_add(length, name.as_os_str().as_bytes().len())?;
            length = checked_wire_add(length, 4)?;
            length = checked_wire_add(length, value.as_os_str().as_bytes().len())?;
        }
        if length > MAX_PAYLOAD {
            return Err(invalid("payload exceeds 64 KiB"));
        }
        Ok(length)
    }
}

fn checked_wire_add(current: usize, additional: usize) -> Result<usize, ProfileError> {
    current
        .checked_add(additional)
        .ok_or_else(|| invalid("runtime service payload length overflow"))
}

fn reject_nul(bytes: &[u8], field: &'static str) -> Result<(), ProfileError> {
    if bytes.contains(&0) {
        return Err(invalid(match field {
            "argument" => "NUL in argument",
            "environment key" => "NUL in environment key",
            "environment value" => "NUL in environment value",
            _ => "NUL in field",
        }));
    }
    Ok(())
}

/// Readiness notification for an already-started runtime instance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadyRequest {
    pub token: InstanceToken,
    pub readiness: ReadinessMask,
}

/// Notification that an already-started runtime service lost one or more
/// transport capabilities.  The profile authenticates the sending peer and
/// token; this request never carries a caller-supplied PID or key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LossRequest {
    pub token: InstanceToken,
    pub lost: ReadinessMask,
}

impl LossRequest {
    pub fn encode(&self) -> Result<Vec<u8>, ProfileError> {
        ReadinessMask::new(self.lost.bits())?;
        let mut writer = Writer::new();
        writer.u8(VERSION);
        writer.fixed(&self.token.0);
        writer.u32(self.lost.bits());
        writer.finish()
    }

    pub fn decode(payload: &[u8]) -> Result<Self, ProfileError> {
        let mut reader = Reader::new(payload)?;
        reader.version()?;
        let mut token = [0_u8; 16];
        reader.fixed(&mut token)?;
        let lost = ReadinessMask::new(reader.u32()?)?;
        reader.finish()?;
        Ok(Self {
            token: InstanceToken(token),
            lost,
        })
    }
}

impl ReadyRequest {
    pub fn encode(&self) -> Result<Vec<u8>, ProfileError> {
        ReadinessMask::new(self.readiness.bits())?;
        let mut writer = Writer::new();
        writer.u8(VERSION);
        writer.fixed(&self.token.0);
        writer.u32(self.readiness.bits());
        writer.finish()
    }

    pub fn decode(payload: &[u8]) -> Result<Self, ProfileError> {
        let mut reader = Reader::new(payload)?;
        reader.version()?;
        let mut token = [0_u8; 16];
        reader.fixed(&mut token)?;
        let readiness = ReadinessMask::new(reader.u32()?)?;
        reader.finish()?;
        Ok(Self {
            token: InstanceToken(token),
            readiness,
        })
    }
}

/// Successful start response.  The PID is validated against the Android
/// signed-32-bit PID contract; the daemon owns the token.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StartRuntimeResponse {
    pub pid: u32,
    pub token: InstanceToken,
}

impl StartRuntimeResponse {
    pub fn encode(&self) -> Result<Vec<u8>, ProfileError> {
        validate_pid(self.pid)?;
        let mut writer = Writer::new();
        writer.u8(VERSION);
        writer.u32(self.pid);
        writer.fixed(&self.token.0);
        writer.finish()
    }

    pub fn decode(payload: &[u8]) -> Result<Self, ProfileError> {
        let mut reader = Reader::new(payload)?;
        reader.version()?;
        let pid = reader.u32()?;
        validate_pid(pid)?;
        let mut token = [0_u8; 16];
        reader.fixed(&mut token)?;
        reader.finish()?;
        Ok(Self {
            pid,
            token: InstanceToken(token),
        })
    }
}

fn validate_pid(pid: u32) -> Result<(), ProfileError> {
    if pid == 0 || pid > MAX_PID {
        return Err(invalid("PID must be in 1..=i32::MAX"));
    }
    Ok(())
}

struct Writer {
    payload: Vec<u8>,
}

impl Writer {
    fn new() -> Self {
        Self::with_capacity(0)
    }

    fn with_capacity(capacity: usize) -> Self {
        Self {
            payload: Vec::with_capacity(capacity),
        }
    }

    fn u8(&mut self, value: u8) {
        self.payload.push(value);
    }

    fn u16(&mut self, value: u16) {
        self.payload.extend_from_slice(&value.to_le_bytes());
    }

    fn u32(&mut self, value: u32) {
        self.payload.extend_from_slice(&value.to_le_bytes());
    }

    fn fixed(&mut self, value: &[u8]) {
        self.payload.extend_from_slice(value);
    }

    fn bytes(&mut self, value: &[u8]) {
        self.u32(value.len() as u32);
        self.fixed(value);
    }

    fn finish(self) -> Result<Vec<u8>, ProfileError> {
        if self.payload.len() > MAX_PAYLOAD {
            Err(invalid("payload exceeds 64 KiB"))
        } else {
            Ok(self.payload)
        }
    }
}

struct Reader<'a> {
    payload: &'a [u8],
    cursor: usize,
}

impl<'a> Reader<'a> {
    fn new(payload: &'a [u8]) -> Result<Self, ProfileError> {
        if payload.len() > MAX_PAYLOAD {
            return Err(invalid("payload exceeds 64 KiB"));
        }
        Ok(Self { payload, cursor: 0 })
    }

    fn version(&mut self) -> Result<(), ProfileError> {
        if self.u8()? != VERSION {
            return Err(invalid("unsupported runtime service protocol version"));
        }
        Ok(())
    }

    fn u8(&mut self) -> Result<u8, ProfileError> {
        let value = *self
            .payload
            .get(self.cursor)
            .ok_or_else(|| invalid("truncated runtime service payload"))?;
        self.cursor += 1;
        Ok(value)
    }

    fn u16(&mut self) -> Result<u16, ProfileError> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, ProfileError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn fixed(&mut self, output: &mut [u8]) -> Result<(), ProfileError> {
        output.copy_from_slice(self.take(output.len())?);
        Ok(())
    }

    fn bytes(&mut self) -> Result<&'a [u8], ProfileError> {
        let length = self.u32()? as usize;
        self.take(length)
    }

    fn string(&mut self) -> Result<String, ProfileError> {
        let value = self.bytes()?;
        String::from_utf8(value.to_vec()).map_err(|_| invalid("package is not UTF-8"))
    }

    fn os_string(&mut self) -> Result<OsString, ProfileError> {
        let value = self.bytes()?;
        if value.contains(&0) {
            return Err(invalid("NUL in runtime service field"));
        }
        Ok(OsString::from_vec(value.to_vec()))
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], ProfileError> {
        let end = self
            .cursor
            .checked_add(length)
            .ok_or_else(|| invalid("runtime service length overflow"))?;
        let value = self
            .payload
            .get(self.cursor..end)
            .ok_or_else(|| invalid("truncated runtime service payload"))?;
        self.cursor = end;
        Ok(value)
    }

    fn finish(&self) -> Result<(), ProfileError> {
        if self.cursor == self.payload.len() {
            Ok(())
        } else {
            Err(invalid("trailing runtime service payload"))
        }
    }
}

#[cfg(test)]
#[path = "runtime_service_protocol_tests.rs"]
mod tests;
