//! Typed, daemon-owned launch contract for a bound Android service process.
//!
//! Android's system process owns the service record and lifecycle policy.  The
//! profile daemon owns only the Darwin mechanism: it validates the bounded
//! request, spawns one child, retains its `Child` and reaps it.  No descriptor
//! or Binder capability is returned to the requester.

use crate::process_command::prepare_command;
use crate::process_incarnation::ProcessIncarnation;
use crate::{ProfileError, registry::validate_package};
use std::ffi::OsString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::Child;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const VERSION: u8 = 2;
const MAX_PAYLOAD: usize = 64 * 1024;
const MAX_ARGC: usize = 64;
const MAX_ENVC: usize = 256;
const MAX_PID: u32 = i32::MAX as u32;
const FIRST_ISOLATED_UID: u32 = 99_000;
const LAST_ISOLATED_UID: u32 = 99_999;
const ACTIVATION_TIMEOUT: Duration = Duration::from_secs(30);

fn invalid(detail: &'static str) -> ProfileError {
    ProfileError::Daemon(format!("invalid bound-service process request: {detail}"))
}

/// Android-owned identity for one active bound-service process slot.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct BoundServiceIdentity {
    pub(crate) package: String,
    pub(crate) process_name: String,
    pub(crate) uid: u32,
    pub(crate) isolated: bool,
}

/// Request sent by the authenticated `android.system` process.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundServiceProcessRequest {
    pub package: String,
    pub process_name: String,
    pub uid: u32,
    pub isolated: bool,
    pub start_sequence: u64,
}

/// Daemon-internal host launch configuration. This never crosses the
/// system-server wire boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BoundServiceProcessLaunch {
    pub(crate) request: BoundServiceProcessRequest,
    pub(crate) arguments: Vec<OsString>,
    pub(crate) environment: Vec<(OsString, OsString)>,
}

impl BoundServiceProcessRequest {
    pub(crate) fn identity(&self) -> BoundServiceIdentity {
        BoundServiceIdentity {
            package: self.package.clone(),
            process_name: self.process_name.clone(),
            uid: self.uid,
            isolated: self.isolated,
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, ProfileError> {
        self.validate()?;
        let mut writer = Writer::with_capacity(self.wire_len()?);
        writer.u8(VERSION);
        writer.string(&self.package);
        writer.string(&self.process_name);
        writer.u32(self.uid);
        writer.u8(u8::from(self.isolated));
        writer.u64(self.start_sequence);
        writer.finish()
    }

    pub fn decode(payload: &[u8]) -> Result<Self, ProfileError> {
        let mut reader = Reader::new(payload)?;
        if reader.u8()? != VERSION {
            return Err(invalid("unsupported version"));
        }
        let package = reader.string()?;
        let process_name = reader.string()?;
        let uid = reader.u32()?;
        let isolated = match reader.u8()? {
            0 => false,
            1 => true,
            _ => return Err(invalid("isolated flag is not boolean")),
        };
        let start_sequence = reader.u64()?;
        reader.finish()?;
        let request = Self {
            package,
            process_name,
            uid,
            isolated,
            start_sequence,
        };
        request.validate()?;
        Ok(request)
    }

    fn validate(&self) -> Result<(), ProfileError> {
        validate_package(&self.package)?;
        validate_process_name(&self.process_name)?;
        if self.isolated {
            if !(FIRST_ISOLATED_UID..=LAST_ISOLATED_UID).contains(&self.uid) {
                return Err(invalid("isolated UID is outside Android's isolated range"));
            }
        } else if self.uid != 1000 && !(10_000..=19_999).contains(&self.uid) {
            return Err(invalid("non-isolated UID is outside the Android app range"));
        }
        Ok(())
    }

    fn wire_len(&self) -> Result<usize, ProfileError> {
        let mut length = 1_usize;
        for value in [&self.package, &self.process_name] {
            length = checked_add(length, 4 + value.len())?;
        }
        length = checked_add(length, 4 + 1 + 8)?;
        if length > MAX_PAYLOAD {
            return Err(invalid("payload exceeds 64 KiB"));
        }
        Ok(length)
    }
}

impl BoundServiceProcessLaunch {
    pub(crate) fn validate(&self) -> Result<(), ProfileError> {
        self.request.validate()?;
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
}

fn validate_process_name(name: &str) -> Result<(), ProfileError> {
    if name.is_empty()
        || name.len() > 255
        || name
            .bytes()
            .any(|byte| byte == 0 || byte == b'\n' || byte == b'\r')
    {
        return Err(invalid(
            "process name is empty, too large, or contains a control byte",
        ));
    }
    Ok(())
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

fn checked_add(current: usize, additional: usize) -> Result<usize, ProfileError> {
    current
        .checked_add(additional)
        .ok_or_else(|| invalid("payload length overflow"))
}

/// Daemon-owned, PID-reuse-safe handle for one prepared child. The opaque
/// token authorizes only the one-shot activation operation; it is not a
/// control FD, Binder endpoint, or general process-spawn capability.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BoundServiceProcessResponse {
    pub pid: u32,
    pub start_sequence: u64,
    pub incarnation: [u64; 2],
    pub activation_token: [u8; 16],
}

impl BoundServiceProcessResponse {
    pub fn encode(self) -> Result<Vec<u8>, ProfileError> {
        if self.pid == 0 || self.pid > MAX_PID {
            return Err(invalid("PID must be in 1..=i32::MAX"));
        }
        let mut writer = Writer::new();
        writer.u8(VERSION);
        writer.u32(self.pid);
        writer.u64(self.start_sequence);
        writer.u64(self.incarnation[0]);
        writer.u64(self.incarnation[1]);
        writer.fixed(&self.activation_token);
        writer.finish()
    }

    pub fn decode(payload: &[u8]) -> Result<Self, ProfileError> {
        let mut reader = Reader::new(payload)?;
        if reader.u8()? != VERSION {
            return Err(invalid("unsupported version"));
        }
        let pid = reader.u32()?;
        if pid == 0 || pid > MAX_PID {
            return Err(invalid("PID must be in 1..=i32::MAX"));
        }
        let start_sequence = reader.u64()?;
        let incarnation = [reader.u64()?, reader.u64()?];
        let mut activation_token = [0_u8; 16];
        reader.fixed(&mut activation_token)?;
        reader.finish()?;
        Ok(Self {
            pid,
            start_sequence,
            incarnation,
            activation_token,
        })
    }
}

/// A daemon-owned child.  The process lease callback is run only after the
/// child has been waited, including the spawn-worker failure/drop path.
pub(crate) struct OwnedBoundServiceChild {
    child: Child,
    activation: Arc<Mutex<Option<crate::process_start_gate::StartGate>>>,
    on_exit: Option<Box<dyn FnOnce() + Send>>,
}

impl OwnedBoundServiceChild {
    pub(crate) fn new(
        child: Child,
        activation: Arc<Mutex<Option<crate::process_start_gate::StartGate>>>,
        on_exit: Box<dyn FnOnce() + Send>,
    ) -> Self {
        Self {
            child,
            activation,
            on_exit: Some(on_exit),
        }
    }

    pub(crate) fn activation(&self) -> Arc<Mutex<Option<crate::process_start_gate::StartGate>>> {
        Arc::clone(&self.activation)
    }

    pub(crate) fn activate(
        activation: &Arc<Mutex<Option<crate::process_start_gate::StartGate>>>,
    ) -> Result<(), ProfileError> {
        let gate = activation
            .lock()
            .map_err(|_| ProfileError::Daemon("bound-service activation lock poisoned".into()))?
            .take()
            .ok_or_else(|| invalid("bound-service process is already activated or expired"))?;
        gate.release()
            .map_err(|error| ProfileError::Daemon(format!("bound-service activation: {error}")))
    }

    pub(crate) fn supervise(
        mut self,
        after_exit: impl FnOnce() + Send + 'static,
    ) -> Result<(), ProfileError> {
        let process_exit = self
            .on_exit
            .take()
            .expect("bound-service child must have a process callback");
        self.on_exit = Some(Box::new(move || {
            process_exit();
            after_exit();
        }));
        thread::Builder::new()
            .name("bound-service-child".into())
            .spawn(move || {
                let deadline = Instant::now() + ACTIVATION_TIMEOUT;
                loop {
                    match self.child.try_wait() {
                        Ok(Some(_)) | Err(_) => break,
                        Ok(None) => {}
                    }
                    let waiting = self
                        .activation
                        .lock()
                        .map(|gate| gate.is_some())
                        .unwrap_or(false);
                    if waiting && Instant::now() >= deadline {
                        let _ = self.activation.lock().map(|mut gate| gate.take());
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                if let Some(on_exit) = self.on_exit.take() {
                    on_exit();
                }
            })
            .map(|_| ())
            .map_err(|error| ProfileError::Daemon(format!("bound-service supervisor: {error}")))
    }
}

impl Drop for OwnedBoundServiceChild {
    fn drop(&mut self) {
        let live = match self.child.try_wait() {
            Ok(Some(_)) => false,
            Ok(None) | Err(_) => true,
        };
        if live {
            let _ = self.activation.lock().map(|mut gate| gate.take());
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        if let Some(on_exit) = self.on_exit.take() {
            on_exit();
        }
    }
}

pub(crate) fn spawn(
    launch: &BoundServiceProcessLaunch,
    working_directory: &Path,
    on_child_registered: impl FnOnce(u32) -> Result<Box<dyn FnOnce() + Send>, ProfileError>,
) -> Result<(BoundServiceProcessResponse, OwnedBoundServiceChild), ProfileError> {
    launch.validate()?;
    let request = &launch.request;
    let mut command = prepare_command(&launch.arguments, &launch.environment, working_directory)?;
    // Do not let a service child attach or publish Binder state before the
    // daemon has installed its authenticated PID/incarnation owner record.
    let gate = crate::process_start_gate::StartGate::prepare(&mut command)?;
    let activation = Arc::new(Mutex::new(Some(gate)));
    let child = command.spawn()?;
    let pid = child.id();
    let on_exit = match on_child_registered(pid) {
        Ok(on_exit) => on_exit,
        Err(error) => {
            let mut child = child;
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    let incarnation = match ProcessIncarnation::read(pid) {
        Ok(incarnation) => incarnation,
        Err(error) => {
            let mut child = child;
            let _ = child.kill();
            let _ = child.wait();
            on_exit();
            return Err(error);
        }
    };
    let activation_token = match generate_activation_token() {
        Ok(token) => token,
        Err(error) => {
            let mut child = child;
            let _ = child.kill();
            let _ = child.wait();
            on_exit();
            return Err(error);
        }
    };
    Ok((
        BoundServiceProcessResponse {
            pid,
            start_sequence: request.start_sequence,
            incarnation: incarnation.parts(),
            activation_token,
        },
        OwnedBoundServiceChild::new(child, activation, on_exit),
    ))
}

fn generate_activation_token() -> Result<[u8; 16], ProfileError> {
    let mut token = [0_u8; 16];
    let mut random = std::fs::File::open("/dev/urandom")?;
    std::io::Read::read_exact(&mut random, &mut token)?;
    if token == [0; 16] {
        return Err(invalid("activation token generation returned zero"));
    }
    Ok(token)
}

struct Writer {
    payload: Vec<u8>,
}

impl Writer {
    fn new() -> Self {
        Self {
            payload: Vec::new(),
        }
    }
    fn with_capacity(capacity: usize) -> Self {
        Self {
            payload: Vec::with_capacity(capacity),
        }
    }
    fn u8(&mut self, value: u8) {
        self.payload.push(value);
    }
    fn u32(&mut self, value: u32) {
        self.payload.extend_from_slice(&value.to_le_bytes());
    }
    fn u64(&mut self, value: u64) {
        self.payload.extend_from_slice(&value.to_le_bytes());
    }
    fn fixed(&mut self, value: &[u8]) {
        self.payload.extend_from_slice(value);
    }
    fn bytes(&mut self, value: &[u8]) {
        self.u32(value.len() as u32);
        self.payload.extend_from_slice(value);
    }
    fn string(&mut self, value: &str) {
        self.bytes(value.as_bytes());
    }
    fn finish(self) -> Result<Vec<u8>, ProfileError> {
        if self.payload.len() > MAX_PAYLOAD {
            return Err(invalid("payload exceeds 64 KiB"));
        }
        Ok(self.payload)
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
    fn take(&mut self, length: usize) -> Result<&'a [u8], ProfileError> {
        let end = self
            .cursor
            .checked_add(length)
            .filter(|end| *end <= self.payload.len())
            .ok_or_else(|| invalid("truncated payload"))?;
        let value = &self.payload[self.cursor..end];
        self.cursor = end;
        Ok(value)
    }
    fn u8(&mut self) -> Result<u8, ProfileError> {
        Ok(*self.take(1)?.first().unwrap())
    }
    fn u32(&mut self) -> Result<u32, ProfileError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, ProfileError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn fixed(&mut self, output: &mut [u8]) -> Result<(), ProfileError> {
        output.copy_from_slice(self.take(output.len())?);
        Ok(())
    }
    fn bytes(&mut self) -> Result<&'a [u8], ProfileError> {
        let length = self.u32()? as usize;
        let value = self.take(length)?;
        if value.contains(&0) {
            return Err(invalid("NUL in field"));
        }
        Ok(value)
    }
    fn string(&mut self) -> Result<String, ProfileError> {
        String::from_utf8(self.bytes()?.to_vec()).map_err(|_| invalid("string is not UTF-8"))
    }
    fn finish(&self) -> Result<(), ProfileError> {
        if self.cursor == self.payload.len() {
            Ok(())
        } else {
            Err(invalid("trailing payload"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::ffi::OsStringExt;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    fn request() -> BoundServiceProcessRequest {
        BoundServiceProcessRequest {
            package: "org.example.app".into(),
            process_name: "org.example.app:renderer".into(),
            uid: 10_042,
            isolated: false,
            start_sequence: 77,
        }
    }

    fn launch() -> BoundServiceProcessLaunch {
        BoundServiceProcessLaunch {
            request: request(),
            arguments: vec!["/bin/cat".into(), "--service".into()],
            environment: vec![("LANG".into(), "C".into())],
        }
    }

    #[test]
    fn request_round_trips_only_android_process_identity() {
        let value = request();
        let encoded = value.encode().unwrap();
        assert_eq!(BoundServiceProcessRequest::decode(&encoded).unwrap(), value);
        assert_eq!(encoded.len(), 1 + 4 + 15 + 4 + 24 + 4 + 1 + 8);
    }

    #[test]
    fn validation_rejects_uid_confusion_and_unbounded_fields() {
        let mut value = request();
        value.uid = 1000;
        assert!(value.encode().is_ok());
        value.isolated = true;
        assert!(value.encode().is_err());
        value.uid = 99_001;
        assert!(value.encode().is_ok());
        value.process_name = "bad\nname".into();
        assert!(value.encode().is_err());
        value.process_name = "service".into();
        let mut launch = launch();
        launch.environment.push(("LANG".into(), "C".into()));
        assert!(launch.validate().is_err());
        launch.environment.pop();
        launch.arguments.push(OsString::from_vec(vec![b'a', 0xff]));
        assert!(launch.validate().is_ok());
    }

    #[test]
    fn response_round_trips_incarnation_safe_activation_handle() {
        let response = BoundServiceProcessResponse {
            pid: 123,
            start_sequence: 9,
            incarnation: [4, 5],
            activation_token: [6; 16],
        };
        let encoded = response.encode().unwrap();
        assert_eq!(encoded.len(), 45);
        assert_eq!(
            BoundServiceProcessResponse::decode(&encoded).unwrap(),
            response
        );
    }

    #[test]
    fn owned_child_reaps_on_supervisor_spawn_failure_path() {
        let mut child = std::process::Command::new("/bin/cat").spawn().unwrap();
        let _ = child.kill();
        let _ = child.wait();
        let _owned =
            OwnedBoundServiceChild::new(child, Arc::new(Mutex::new(None)), Box::new(|| {}));
    }

    #[test]
    #[ignore = "subprocess fixture for the two-phase activation test"]
    fn activation_child_fixture() {
        println!("BEFORE");
        std::io::stdout().flush().unwrap();
        // SAFETY: this isolated subprocess has not started application
        // threads and is the only code accessing its startup environment.
        let result = unsafe { crate::wait_for_process_registration() };
        println!("AFTER {}", result.is_ok());
    }

    #[test]
    fn prepared_child_waits_for_exactly_one_activation() {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "bound_service_process::tests::activation_child_fixture",
                "--ignored",
                "--nocapture",
            ])
            .stdout(Stdio::piped());
        let gate = crate::process_start_gate::StartGate::prepare(&mut command).unwrap();
        let activation = Arc::new(Mutex::new(Some(gate)));
        let mut child = command.spawn().unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        while !line.contains("BEFORE") {
            line.clear();
            assert!(output.read_line(&mut line).unwrap() > 0);
        }

        let (exited_tx, exited_rx) = mpsc::channel();
        let owned = OwnedBoundServiceChild::new(
            child,
            Arc::clone(&activation),
            Box::new(move || exited_tx.send(()).unwrap()),
        );
        owned.supervise(|| {}).unwrap();
        assert!(exited_rx.recv_timeout(Duration::from_millis(50)).is_err());
        OwnedBoundServiceChild::activate(&activation).unwrap();
        assert!(OwnedBoundServiceChild::activate(&activation).is_err());
        exited_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    }
}
