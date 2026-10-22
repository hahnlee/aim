//! Per-generation Linux boot identity owned by the authenticated native init.
use crate::process_namespace::{InitRegistration, ProcessIdentity};
use std::{
    fs,
    io::{self, Read, Write},
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::Path,
};

const FILE: &str = "boot-id";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootId {
    pub owner: ProcessIdentity,
    pub uuid: String,
}

impl BootId {
    pub fn generate(owner: ProcessIdentity) -> io::Result<Self> {
        if !owner.is_live() {
            return Err(io::Error::from_raw_os_error(libc::ESRCH));
        }
        let mut bytes = [0u8; 16];
        if unsafe { libc::getentropy(bytes.as_mut_ptr().cast(), bytes.len()) } < 0 {
            return Err(io::Error::last_os_error());
        }
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let hex = bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let uuid = format!(
            "{}-{}-{}-{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..]
        );
        Ok(Self { owner, uuid })
    }
    pub fn publish(&self, runtime: &Path) -> io::Result<()> {
        if !self.owner.is_live() || !valid_uuid(&self.uuid) {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }
        let text = format!(
            "AIMBOOTID1\t{}\t{}\t{}\t{}\n",
            self.owner.host_pid, self.owner.start_seconds, self.owner.start_microseconds, self.uuid
        );
        let mut file = crate::private_fd::PrivateFile::allocate(|| {
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o400)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(runtime.join(FILE))
        })?;
        if unsafe { libc::fchmod(file.as_raw_fd(), 0o400) } < 0 {
            return Err(io::Error::last_os_error());
        }
        file.write_all(text.as_bytes())?;
        file.sync_all()
    }
    pub fn read(runtime: &Path) -> io::Result<Self> {
        let mut file = crate::private_fd::PrivateFile::allocate(|| {
            fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(runtime.join(FILE))
        })?;
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(file.as_raw_fd(), &mut stat) } < 0 {
            return Err(io::Error::last_os_error());
        }
        if stat.st_mode & libc::S_IFMT != libc::S_IFREG
            || stat.st_nlink != 1
            || stat.st_uid != unsafe { libc::geteuid() }
            || stat.st_mode & 0o777 != 0o400
        {
            return Err(io::Error::from_raw_os_error(libc::EPROTO));
        }
        let mut text = String::new();
        Read::by_ref(&mut file)
            .take(257)
            .read_to_string(&mut text)?;
        if text.len() > 256 || !text.ends_with('\n') || text[..text.len() - 1].contains('\n') {
            return Err(io::Error::from_raw_os_error(libc::EPROTO));
        }
        let fields = text[..text.len() - 1].split('\t').collect::<Vec<_>>();
        if fields.len() != 5 || fields[0] != "AIMBOOTID1" || !valid_uuid(fields[4]) {
            return Err(io::Error::from_raw_os_error(libc::EPROTO));
        }
        let owner = ProcessIdentity {
            host_pid: fields[1]
                .parse()
                .map_err(|_| io::Error::from_raw_os_error(libc::EPROTO))?,
            start_seconds: fields[2]
                .parse()
                .map_err(|_| io::Error::from_raw_os_error(libc::EPROTO))?,
            start_microseconds: fields[3]
                .parse()
                .map_err(|_| io::Error::from_raw_os_error(libc::EPROTO))?,
        };
        let init = InitRegistration::read(&runtime.join("identity/by-pid"))?;
        if owner != init.process {
            return Err(io::Error::from_raw_os_error(libc::EPERM));
        }
        Ok(Self {
            owner,
            uuid: fields[4].into(),
        })
    }
}

fn valid_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(at, byte)| {
            if matches!(at, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')
            }
        })
        && value.as_bytes()[14] == b'4'
        && matches!(value.as_bytes()[19], b'8' | b'9' | b'a' | b'b')
}

#[cfg(test)]
mod tests {
    use super::*;
    fn root(name: &str) -> std::path::PathBuf {
        crate::scratch_dir(&format!("aim-boot-id-{name}"))
    }
    fn registered(runtime: &Path) -> (ProcessIdentity, InitRegistration) {
        let table = runtime.join("identity/by-pid");
        let owner = ProcessIdentity::running_strict(std::process::id() as i32).unwrap();
        let registration = InitRegistration::register(&table, owner, "boot-id-test").unwrap();
        (owner, registration)
    }
    #[test]
    fn each_authenticated_runtime_gets_one_stable_distinct_id() {
        let first = root("first");
        let second = root("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        let (first_owner, first_registration) = registered(&first);
        let (second_owner, second_registration) = registered(&second);
        let first_id = BootId::generate(first_owner).unwrap();
        first_id.publish(&first).unwrap();
        let second_id = BootId::generate(second_owner).unwrap();
        second_id.publish(&second).unwrap();
        assert_eq!(BootId::read(&first).unwrap(), first_id);
        assert_eq!(BootId::read(&first).unwrap(), first_id);
        assert_ne!(first_id.uuid, second_id.uuid);
        assert_eq!(BootId::read(&second).unwrap(), second_id);
        first_registration
            .remove(&first.join("identity/by-pid"))
            .unwrap();
        second_registration
            .remove(&second.join("identity/by-pid"))
            .unwrap();
        fs::remove_dir_all(first).unwrap();
        fs::remove_dir_all(second).unwrap();
    }
    #[test]
    fn record_rejects_a_different_init_owner() {
        let runtime = root("owner");
        fs::create_dir_all(&runtime).unwrap();
        let (owner, registration) = registered(&runtime);
        let id = BootId::generate(owner).unwrap();
        id.publish(&runtime).unwrap();
        let path = runtime.join(FILE);
        let text = fs::read_to_string(&path).unwrap().replacen(
            &format!("\t{}\t", owner.start_microseconds),
            &format!("\t{}\t", owner.start_microseconds.wrapping_add(1)),
            1,
        );
        fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
        fs::write(&path, text).unwrap();
        fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o400)).unwrap();
        assert_eq!(
            BootId::read(&runtime).unwrap_err().raw_os_error(),
            Some(libc::EPERM)
        );
        registration
            .remove(&runtime.join("identity/by-pid"))
            .unwrap();
        fs::remove_dir_all(runtime).unwrap();
    }
}
