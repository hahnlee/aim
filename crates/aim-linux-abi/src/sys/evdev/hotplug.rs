//! Consume the device owner's lifetime records with a boot-shared cursor.
//! Each listening process follows them; flock makes each transition emit
//! once, including across exec and when a former listener exits.

use std::io::{self, Read, Seek, Write};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use aim_host_display::input::hotplug;
use sha2::Digest;

use super::{event_device, input_device};
use crate::sys::{fork, netif, uevent};

fn follow(dir: &Path, cursor: &Path) -> io::Result<()> {
    let _own = fork::spawn::own_fds();
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(cursor)?;
    // SAFETY: our own cursor file; closing it releases the lock.
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes)?;
    let initial = bytes.is_empty();
    let at = if initial {
        0
    } else {
        u64::from_le_bytes(
            bytes
                .as_slice()
                .try_into()
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "input hotplug cursor"))?,
        )
    };
    let (next, records) = hotplug::read(dir, at)?;
    if initial || next != at {
        f.rewind()?;
        f.write_all(&next.to_le_bytes())?;
    }
    for (added, desc) in records {
        let input = input_device(&desc);
        let event = event_device(desc.index);
        let (action, devices) = if added {
            ("add", [input, event])
        } else {
            ("remove", [event, input])
        };
        for dev in devices {
            uevent::emit_locked(action, &dev, &[], &_own);
        }
    }
    Ok(())
}

fn cursor(dir: &Path) -> std::path::PathBuf {
    let hash = sha2::Sha256::digest(dir.as_os_str().as_encoded_bytes());
    let tag: String = hash[..12].iter().map(|b| format!("{b:02x}")).collect();
    netif::kernel_dir("uevent").join(format!("input-{tag}"))
}

pub(crate) fn refresh() -> io::Result<()> {
    match crate::vfs::input_dir() {
        Some(dir) => follow(dir, &cursor(dir)),
        None => Ok(()),
    }
}

pub(crate) fn watch() -> io::Result<()> {
    static STARTED: Mutex<bool> = Mutex::new(false);
    let Some(dir) = crate::vfs::input_dir() else {
        return Ok(());
    };
    let mut started = STARTED.lock().unwrap();
    if *started {
        return Ok(());
    }
    let cursor = cursor(dir);
    follow(dir, &cursor)?;
    std::thread::Builder::new()
        .name("linux-abi-input".into())
        .spawn(move || {
            // SAFETY: host worker threads must not receive guest signals.
            unsafe {
                let mut all: libc::sigset_t = 0;
                libc::sigfillset(&mut all);
                libc::pthread_sigmask(libc::SIG_BLOCK, &all, std::ptr::null_mut());
            }
            loop {
                std::thread::sleep(Duration::from_millis(20));
                if let Err(e) = follow(dir, &cursor) {
                    crate::diag!("[linux-abi] input hotplug: {e}");
                }
            }
        })?;
    *started = true;
    Ok(())
}
