#![forbid(unsafe_op_in_unsafe_fn)]
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

#[cfg(not(target_os = "macos"))]
compile_error!("darwin-art-profile requires macOS; no weaker filesystem fallback exists");

mod app_ids;
mod application_launch_template;
mod binder_capability_service;
mod binder_capability_wire;
mod binder_client;
mod binder_service;
mod binder_transfer;
mod bound_service_child_control;
mod bound_service_client;
mod bound_service_process;
mod bound_service_registry;
mod package_list;
pub use binder_client::{BinderAuthorityConnection, connect_binder_authority_at};
pub use bound_service_client::{
    activate_bound_service_process, cancel_bound_service_process, start_bound_service_process,
};
pub use bound_service_process::{BoundServiceProcessRequest, BoundServiceProcessResponse};
#[cfg(target_os = "macos")]
pub use darwin_art_scm_transfer::inheritance::with_native_operation;
pub use darwin_art_scm_transfer::inheritance::{output_owned, spawn_owned};
mod fd_passing;
mod host_commands;
mod installd;
mod property_service;
pub use host_commands::{HostCommandListener, run_host_command_at};
mod filesystem;
mod host_fd_delivery;
mod package_layout;
mod scm_service;
mod settings_migration;
pub use scm_service::NativeScmEndpointProvider;
mod daemonized_child_wait;
mod listener_wait;
mod peer_credentials;
mod peer_process;
mod process_command;
mod process_identity;
mod process_incarnation;
mod process_start_gate;
mod process_wait;
pub use process_start_gate::wait_for_process_registration;
mod package_client;
pub use package_client::{InstalldRequest, installd_at, property_set_at};
mod process_registry;
mod protocol;
pub use process_identity::ProcessIdentity;
mod registry;
pub mod runtime_service_cli;
mod runtime_service_client;
pub mod runtime_service_endpoints;
mod runtime_service_launch;
pub mod runtime_service_protocol;
mod runtime_service_state;
mod unix_connect;
pub use runtime_service_client::{
    publish_runtime_lost, publish_runtime_ready, start_runtime_service,
};
mod server;

use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io;
use std::mem;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub use server::{DaemonConfig, run_daemon};

pub const PROFILE_ID_ENV: &str = "DARWIN_ART_PROFILE";
pub const PROFILE_ROOT_ENV: &str = "DARWIN_ART_PROFILE_ROOT";
pub const PROFILE_SOCKET_ENV: &str = "DARWIN_ART_PROFILE_SOCKET";

#[derive(Debug)]
pub enum ProfileError {
    InvalidProfileId,
    MissingHome,
    InvalidResponse(String),
    Daemon(String),
    Io(io::Error),
}

impl fmt::Display for ProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidProfileId => write!(
                formatter,
                "profile id must be 1-48 lowercase ASCII letters, digits, '.' '_' or '-'"
            ),
            Self::MissingHome => write!(formatter, "HOME is unavailable for the profile root"),
            Self::InvalidResponse(message) => {
                write!(formatter, "invalid darwin-artd response: {message}")
            }
            Self::Daemon(message) => write!(formatter, "darwin-artd: {message}"),
            Self::Io(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ProfileError {}

impl From<io::Error> for ProfileError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfilePaths {
    pub profiles_root: PathBuf,
    pub profile_id: String,
    pub profile_root: PathBuf,
    pub socket: PathBuf,
    pub image: PathBuf,
    pub mount: PathBuf,
}

impl ProfilePaths {
    pub fn new(profiles_root: PathBuf, profile_id: &str) -> Result<Self, ProfileError> {
        validate_profile_id(profile_id)?;
        let profile_root = profiles_root.join(profile_id);
        Ok(Self {
            profiles_root,
            profile_id: profile_id.to_owned(),
            socket: profile_root.join("control.sock"),
            image: profile_root.join("android-data.sparsebundle"),
            mount: profile_root.join("mnt"),
            profile_root,
        })
    }

    pub fn from_environment() -> Result<Self, ProfileError> {
        let profile_id = env::var(PROFILE_ID_ENV).unwrap_or_else(|_| "default".to_owned());
        Self::new(default_profiles_root()?, &profile_id)
    }
}

pub fn validate_profile_id(profile_id: &str) -> Result<(), ProfileError> {
    if profile_id.is_empty()
        || profile_id.len() > 48
        || profile_id == "."
        || profile_id == ".."
        || !profile_id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
    {
        return Err(ProfileError::InvalidProfileId);
    }
    Ok(())
}

pub fn default_profiles_root() -> Result<PathBuf, ProfileError> {
    if let Some(root) = env::var_os(PROFILE_ROOT_ENV) {
        return Ok(PathBuf::from(root));
    }
    let home = env::var_os("HOME").ok_or(ProfileError::MissingHome)?;
    Ok(PathBuf::from(home).join("Library/Application Support/DarwinART/profiles"))
}

pub fn list_profile_ids(profiles_root: &Path) -> Result<Vec<String>, ProfileError> {
    let mut profiles = Vec::new();
    match std::fs::read_dir(profiles_root) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                let Some(profile_id) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if entry.file_type()?.is_dir() && validate_profile_id(&profile_id).is_ok() {
                    profiles.push(profile_id);
                }
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    if !profiles.iter().any(|profile| profile == "default") {
        profiles.push("default".to_owned());
    }
    profiles.sort();
    Ok(profiles)
}

pub fn create_profile(
    profiles_root: &Path,
    profile_id: &str,
) -> Result<ProfilePaths, ProfileError> {
    let paths = ProfilePaths::new(profiles_root.to_owned(), profile_id)?;
    reject_profile_control_symlink(profiles_root)?;
    reject_profile_control_symlink(&paths.profile_root)?;
    if paths.profile_root.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "profile already exists").into());
    }
    std::fs::create_dir_all(&paths.profile_root)?;
    std::fs::set_permissions(&paths.profile_root, std::fs::Permissions::from_mode(0o700))?;
    Ok(paths)
}

pub fn delete_profile(profiles_root: &Path, profile_id: &str) -> Result<(), ProfileError> {
    let paths = ProfilePaths::new(profiles_root.to_owned(), profile_id)?;
    reject_profile_control_symlink(profiles_root)?;
    reject_profile_control_symlink(&paths.profile_root)?;
    if !paths.profile_root.exists() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "profile does not exist").into());
    }
    let socket_is_endpoint = std::fs::symlink_metadata(&paths.socket)
        .map(|metadata| metadata.file_type().is_socket())
        .unwrap_or(false);
    let processes = if socket_is_endpoint {
        match list_processes(&paths) {
            Ok(processes) => Some(processes),
            Err(ProfileError::Io(error))
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound
                        | io::ErrorKind::ConnectionRefused
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::NotConnected
                ) =>
            {
                None
            }
            Err(error) => return Err(error),
        }
    } else {
        None
    };
    if let Some(processes) = processes {
        let mut resident_pids = Vec::new();
        for line in processes.lines() {
            let Some((pid, package)) = line.split_once('\t') else {
                return Err(ProfileError::InvalidResponse("bad process listing".into()));
            };
            if package != "android.system" {
                return Err(ProfileError::Daemon(format!(
                    "profile has a running application: {package}"
                )));
            }
            resident_pids.push(pid.to_owned());
        }
        if !resident_pids.is_empty() {
            let status =
                spawn_owned(Command::new("/bin/kill").arg("-TERM").args(&resident_pids))?.wait()?;
            if !status.success() {
                return Err(ProfileError::Daemon(
                    "could not stop profile services".into(),
                ));
            }
            let deadline = Instant::now() + Duration::from_secs(10);
            while !list_processes(&paths)?.is_empty() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(50));
            }
            if !list_processes(&paths)?.is_empty() {
                return Err(ProfileError::Daemon("profile services did not stop".into()));
            }
        }
        shutdown_daemon(&paths)?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while paths.socket.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
        if paths.socket.exists() {
            return Err(ProfileError::Daemon("profile daemon did not stop".into()));
        }
    }
    let filesystem = filesystem::ProfileFilesystem::new(paths.clone());
    filesystem.detach()?;
    std::fs::remove_dir_all(&paths.profile_root)?;
    if profiles_root.exists() {
        std::fs::File::open(profiles_root)?.sync_all()?;
    }
    Ok(())
}

pub fn profile_allocated_bytes(
    profiles_root: &Path,
    profile_id: &str,
) -> Result<u64, ProfileError> {
    let paths = ProfilePaths::new(profiles_root.to_owned(), profile_id)?;
    reject_profile_control_symlink(&paths.profile_root)?;
    let mut total = 0_u64;
    match std::fs::read_dir(&paths.profile_root) {
        Ok(entries) => {
            for entry in entries {
                let path = entry?.path();
                if path != paths.mount {
                    total = total.saturating_add(allocated_bytes(&path)?);
                }
            }
            Ok(total)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error.into()),
    }
}

fn allocated_bytes(path: &Path) -> Result<u64, ProfileError> {
    use std::os::unix::fs::MetadataExt;

    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Ok(0),
        Ok(metadata) if metadata.is_dir() => {
            let mut total = metadata.blocks().saturating_mul(512);
            for entry in std::fs::read_dir(path)? {
                total = total.saturating_add(allocated_bytes(&entry?.path())?);
            }
            Ok(total)
        }
        Ok(metadata) => Ok(metadata.blocks().saturating_mul(512)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error.into()),
    }
}

fn reject_profile_control_symlink(path: &Path) -> Result<(), ProfileError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(ProfileError::Daemon(format!(
            "refusing symlink in profile control path: {}",
            path.display()
        ))),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub struct ProfileLease {
    stream: UnixStream,
}

impl ProfileLease {
    pub fn connect(socket: &Path) -> Result<Self, ProfileError> {
        Self::connect_with_identity(socket, None)
    }

    pub fn connect_process(socket: &Path, package: &str) -> Result<Self, ProfileError> {
        Self::connect_process_pid(socket, std::process::id(), package)
    }

    pub fn connect_process_pid(
        socket: &Path,
        pid: u32,
        package: &str,
    ) -> Result<Self, ProfileError> {
        registry::validate_package(package)?;
        Self::connect_with_identity(socket, Some((pid, package)))
    }

    fn connect_with_identity(
        socket: &Path,
        identity: Option<(u32, &str)>,
    ) -> Result<Self, ProfileError> {
        let mut stream = UnixStream::connect(socket)?;
        let mut payload = Vec::new();
        if let Some((pid, package)) = identity {
            payload.extend_from_slice(&pid.to_le_bytes());
            payload.extend_from_slice(package.as_bytes());
        }
        protocol::write_request(&mut stream, protocol::OP_ACQUIRE, &payload)?;
        protocol::expect_ok(&mut stream, protocol::OP_ACQUIRE)?;
        Ok(Self { stream })
    }

    pub fn connect_from_environment() -> Result<Option<Self>, ProfileError> {
        let Some(socket) = env::var_os(PROFILE_SOCKET_ENV) else {
            return Ok(None);
        };
        match env::var("DARWIN_ART_APK_APP_PACKAGE") {
            Ok(package) => Self::connect_process(Path::new(&socket), &package).map(Some),
            Err(_) => Self::connect(Path::new(&socket)).map(Some),
        }
    }

    /// Keep the lease connection open across a following `exec(2)`.
    ///
    /// The daemon then observes the socket lifetime of the replacement image,
    /// so its registered PID is the Android process rather than a wrapper.
    pub fn preserve_for_exec(self) -> Result<i32, ProfileError> {
        let descriptor = self.stream.as_raw_fd();
        let flags = unsafe { fcntl(descriptor, F_GETFD) };
        if flags < 0 || unsafe { fcntl(descriptor, F_SETFD, flags & !FD_CLOEXEC) } < 0 {
            return Err(io::Error::last_os_error().into());
        }
        mem::forget(self);
        Ok(descriptor)
    }
}

impl Drop for ProfileLease {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}

/// A daemon binary's build identity: its size and modification time. A
/// rebuilt `darwin-artd` differs from the one a running daemon was started from.
pub(crate) fn binary_identity(path: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path).ok()?;
    Some(format!(
        "{}:{}.{:09}",
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec()
    ))
}

fn daemon_binary() -> PathBuf {
    env::var_os("DARWIN_ART_DAEMON")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            env::current_exe()
                .unwrap_or_else(|_| PathBuf::from("darwin-artctl"))
                .with_file_name("darwin-artd")
        })
}

/// Whether the running daemon was started from a different build than the
/// installed `darwin-artd`. A daemon too old to answer is stale as well.
fn running_daemon_is_stale(paths: &ProfilePaths) -> bool {
    let Some(installed) = binary_identity(&daemon_binary()) else {
        return false;
    };
    match request(paths, protocol::OP_BUILD_IDENTITY, &[]) {
        Ok(running) => running != installed.as_bytes(),
        // Unreachable: nothing to compare. Any other failure is a daemon that
        // does not know this operation, i.e. an older build.
        Err(ProfileError::Io(error))
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound
                    | io::ErrorKind::ConnectionRefused
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::BrokenPipe
            ) =>
        {
            false
        }
        Err(_) => true,
    }
}

/// Replaces a stale daemon that serves nobody; a busy one is kept (its
/// processes and leases belong to it) and reported instead of reused silently.
fn replace_stale_daemon(paths: &ProfilePaths) -> Result<bool, ProfileError> {
    if !running_daemon_is_stale(paths) {
        return Ok(false);
    }
    let idle = daemon_status(paths)
        .map(|status| status.split_whitespace().any(|field| field == "leases=0"))
        .unwrap_or(false)
        && list_processes(paths).map(|list| list.trim().is_empty()).unwrap_or(false);
    if !idle {
        eprintln!(
            "darwin-artctl: warning: the running darwin-artd is older than {}; it serves \
             processes, so it is kept. Stop them and run `darwin-artctl shutdown` to update it.",
            daemon_binary().display()
        );
        return Ok(false);
    }
    eprintln!("darwin-artctl: restarting the stale darwin-artd with the installed build");
    // The daemon may close the connection as it exits instead of replying;
    // its socket disappearing is the completion that matters.
    let _ = shutdown_daemon(paths);
    let deadline = Instant::now() + Duration::from_secs(10);
    while paths.socket.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(50));
    }
    Ok(true)
}

pub fn ensure_daemon(paths: &ProfilePaths) -> Result<PathBuf, ProfileError> {
    // Replace a stale daemon before asking it to mount: an older build may
    // fail requests the installed one handles.
    let running = if paths.socket.exists() && replace_stale_daemon(paths)? {
        Err(ProfileError::Io(io::Error::from(io::ErrorKind::NotFound)))
    } else {
        request(paths, protocol::OP_ENSURE, &[])
    };
    match running {
        Ok(bytes) => return Ok(PathBuf::from(OsString::from_vec(bytes))),
        Err(ProfileError::Io(error))
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound
                    | io::ErrorKind::ConnectionRefused
                    | io::ErrorKind::ConnectionReset
            ) => {}
        Err(error) => return Err(error),
    }
    spawn_daemon(paths)?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match request(paths, protocol::OP_ENSURE, &[]) {
            Ok(bytes) => return Ok(PathBuf::from(OsString::from_vec(bytes))),
            Err(ProfileError::Io(error))
                if Instant::now() < deadline
                    && matches!(
                        error.kind(),
                        io::ErrorKind::NotFound
                            | io::ErrorKind::ConnectionRefused
                            | io::ErrorKind::ConnectionReset
                    ) =>
            {
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(error),
        }
    }
}

pub fn daemon_status(paths: &ProfilePaths) -> Result<String, ProfileError> {
    let bytes = request(paths, protocol::OP_STATUS, &[])?;
    String::from_utf8(bytes).map_err(|error| ProfileError::InvalidResponse(error.to_string()))
}

pub fn shutdown_daemon(paths: &ProfilePaths) -> Result<(), ProfileError> {
    request(paths, protocol::OP_SHUTDOWN, &[]).map(|_| ())
}

pub fn resolve_process_identity(
    paths: &ProfilePaths,
    pid: u32,
) -> Result<ProcessIdentity, ProfileError> {
    resolve_process_identity_at(&paths.socket, pid)
}

/// Query an already-running profile daemon. Never spawn or select a profile
/// from the Binder dispatch path; the host provides the socket capability.
pub fn resolve_process_identity_at(
    socket: &Path,
    pid: u32,
) -> Result<ProcessIdentity, ProfileError> {
    if pid == 0 {
        return Err(ProfileError::Daemon("process PID must be non-zero".into()));
    }
    let mut stream = UnixStream::connect(socket)?;
    let timeout = Some(std::time::Duration::from_secs(1));
    stream.set_read_timeout(timeout)?;
    stream.set_write_timeout(timeout)?;
    protocol::write_request(
        &mut stream,
        protocol::OP_PROCESS_IDENTITY,
        &pid.to_le_bytes(),
    )?;
    let response = protocol::expect_ok(&mut stream, protocol::OP_PROCESS_IDENTITY)?;
    ProcessIdentity::decode(&response, pid)
}

/// Installed third-party packages, from PackageManagerService's packages.list.
pub fn list_packages(paths: &ProfilePaths) -> Result<String, ProfileError> {
    let bytes = request(paths, protocol::OP_LIST, &[])?;
    String::from_utf8(bytes).map_err(|error| ProfileError::InvalidResponse(error.to_string()))
}

pub fn list_processes(paths: &ProfilePaths) -> Result<String, ProfileError> {
    let bytes = request(paths, protocol::OP_PROCESSES, &[])?;
    String::from_utf8(bytes).map_err(|error| ProfileError::InvalidResponse(error.to_string()))
}

pub fn daemonize_process(
    paths: &ProfilePaths,
    package: &str,
    arguments: &[std::ffi::OsString],
    environment: &[(std::ffi::OsString, std::ffi::OsString)],
) -> Result<u32, ProfileError> {
    registry::validate_package(package)?;
    if arguments.is_empty() {
        return Err(ProfileError::Daemon("daemonize requires a program".into()));
    }
    let mut payload = Vec::new();
    encode_field(&mut payload, package.as_bytes())?;
    payload.extend_from_slice(&(arguments.len() as u32).to_le_bytes());
    for argument in arguments {
        encode_field(&mut payload, argument.as_encoded_bytes())?;
    }
    payload.extend_from_slice(&(environment.len() as u32).to_le_bytes());
    for (key, value) in environment {
        encode_field(&mut payload, key.as_encoded_bytes())?;
        encode_field(&mut payload, value.as_encoded_bytes())?;
    }
    let response = request(paths, protocol::OP_DAEMONIZE, &payload)?;
    if response.len() != 4 {
        return Err(ProfileError::InvalidResponse("bad daemonized PID".into()));
    }
    Ok(u32::from_le_bytes(response.try_into().unwrap()))
}

fn encode_field(output: &mut Vec<u8>, value: &[u8]) -> Result<(), ProfileError> {
    let length = u32::try_from(value.len())
        .map_err(|_| ProfileError::Daemon("daemonize field is too large".into()))?;
    output.extend_from_slice(&length.to_le_bytes());
    output.extend_from_slice(value);
    Ok(())
}

fn request(paths: &ProfilePaths, operation: u16, payload: &[u8]) -> Result<Vec<u8>, ProfileError> {
    let mut stream = UnixStream::connect(&paths.socket)?;
    protocol::write_request(&mut stream, operation, payload)?;
    protocol::expect_ok(&mut stream, operation).map_err(Into::into)
}

fn spawn_daemon(paths: &ProfilePaths) -> Result<(), ProfileError> {
    std::fs::create_dir_all(&paths.profile_root)?;
    let daemon = daemon_binary();
    let log_path = paths.profile_root.join("darwin-artd.log");
    // A crashed or repeatedly restarted runtime must not turn its diagnostic
    // log into unbounded profile storage. Keep one bounded previous log; the
    // Android data image remains reserved for app state rather than logs.
    const MAX_DAEMON_LOG_BYTES: u64 = 16 * 1024 * 1024;
    if let Ok(metadata) = std::fs::metadata(&log_path)
        && metadata.len() > MAX_DAEMON_LOG_BYTES
    {
        let rotated = paths.profile_root.join("darwin-artd.log.1");
        let _ = std::fs::remove_file(&rotated);
        std::fs::rename(&log_path, rotated)?;
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    let error_log = log.try_clone()?;
    let mut command = Command::new(daemon);
    command
        .arg("--root")
        .arg(&paths.profiles_root)
        .arg("--profile")
        .arg(&paths.profile_id)
        .current_dir(&paths.profile_root)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(error_log));
    // The daemon must outlive the launcher terminal/process group. setsid is
    // the only unsafe seam here and runs after fork using an async-signal-safe
    // POSIX primitive, before exec replaces the child image.
    unsafe {
        command.pre_exec(|| {
            if setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    spawn_owned(&mut command)?;
    Ok(())
}

unsafe extern "C" {
    fn setsid() -> i32;
    fn fcntl(descriptor: i32, command: i32, ...) -> i32;
}

const F_GETFD: i32 = 1;
const F_SETFD: i32 = 2;
const FD_CLOEXEC: i32 = 1;

pub fn write_path(path: &OsStr) -> &[u8] {
    path.as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_ids_are_canonical_on_case_insensitive_hosts() {
        for accepted in ["default", "work-2", "api.36", "test_profile"] {
            assert!(validate_profile_id(accepted).is_ok());
        }
        for rejected in ["", ".", "..", "Default", "a/b", "한글"] {
            assert!(validate_profile_id(rejected).is_err(), "{rejected}");
        }
    }

    #[test]
    fn profile_paths_keep_control_state_outside_the_guest_mount() {
        let paths = ProfilePaths::new(PathBuf::from("/profiles"), "default").unwrap();
        assert_eq!(
            paths.socket,
            PathBuf::from("/profiles/default/control.sock")
        );
        assert_eq!(paths.mount, PathBuf::from("/profiles/default/mnt"));
        assert!(!paths.socket.starts_with(&paths.mount));
    }

    #[test]
    fn profile_catalog_is_canonical_and_reports_allocated_bytes() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("darwin-art-catalog-{nonce}"));
        create_profile(&root, "work").unwrap();
        std::fs::write(root.join("work/payload"), b"profile data").unwrap();
        std::fs::create_dir(root.join("Not-A-Profile")).unwrap();
        assert_eq!(list_profile_ids(&root).unwrap(), ["default", "work"]);
        assert!(profile_allocated_bytes(&root, "work").unwrap() > 0);
        std::fs::write(root.join("work/control.sock"), b"stale").unwrap();
        delete_profile(&root, "work").unwrap();
        assert!(!root.join("work").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
