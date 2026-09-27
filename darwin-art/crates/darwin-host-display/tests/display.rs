//! The display server end to end, from the host side of the module: a
//! buffer in a file (as a memfd is on the host) is imported, presented and
//! captured back, and vsync events arrive at the display's rate.
//!
//! Opens a small window: run it in a logged-in session.

use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use darwin_host_display::{MODULE, set_server};
use darwin_hostcall::display::{
    Buffer, Connect, Event, FN_CONNECT, FN_IMPORT, FN_PRESENT, FN_SET_VSYNC, Import, SetVsync,
    event,
};

const W: u32 = 64;
const H: u32 = 48;
const STRIDE: u32 = 256;
const PAGE: u64 = 16384;

struct Kill(Child);

impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn call<T>(func: u32, args: &mut T) -> i64 {
    // SAFETY: `args` is the function's argument block.
    unsafe { (MODULE.call)(func, args as *mut T as u64, size_of::<T>() as u64) }
}

fn pixel(x: u32, y: u32) -> [u8; 4] {
    [(x * 4) as u8, (y * 5) as u8, 0x80, 0xff]
}

fn wait_for(path: &Path) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(data) = std::fs::read(path)
            && data.len() >= 54 + (W * H * 4) as usize
        {
            return data;
        }
        assert!(
            Instant::now() < deadline,
            "no capture at {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn presents_a_buffer_and_delivers_vsync() {
    let dir =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("display-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // sun_path is 104 bytes on Darwin.
    let socket = std::env::temp_dir().join(format!("dd-{}.sock", std::process::id()));
    let capture = dir.join("capture.bmp");
    let mut server = Command::new(env!("CARGO_BIN_EXE_darwin-display"))
        .args([
            "--size",
            &format!("{W}x{H}"),
            "--title",
            "darwin-display test",
        ])
        .arg("--socket")
        .arg(&socket)
        .arg("--capture")
        .arg(&capture)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut banner = [0u8; 32];
    server
        .stdout
        .as_mut()
        .unwrap()
        .read_exact(&mut banner)
        .unwrap();
    let server = Kill(server);
    set_server(&socket);

    let mut info = Connect::default();
    let fd = call(FN_CONNECT, &mut info);
    assert!(fd >= 0, "connect: {fd}");
    // SAFETY: the module returned a new fd for us.
    let events = unsafe { OwnedFd::from_raw_fd(fd as i32) };
    assert_eq!((info.width, info.height), (W, H));
    assert!(info.vsync_period_ns > 1_000_000, "{info:?}");

    // A buffer: RGBA rows of STRIDE bytes in a page-sized file.
    let file = dir.join("buffer");
    let mut data = vec![0u8; PAGE as usize];
    for y in 0..H {
        for x in 0..W {
            let at = (y * STRIDE + x * 4) as usize;
            data[at..at + 4].copy_from_slice(&pixel(x, y));
        }
    }
    std::fs::write(&file, &data).unwrap();
    let buffer = std::fs::File::open(&file).unwrap();
    let mut import = Import {
        fd: buffer.as_raw_fd(),
        format: 1,
        width: W,
        height: H,
        stride_bytes: STRIDE,
        length: PAGE,
        id: 7,
        ..Default::default()
    };
    assert_eq!(call(FN_IMPORT, &mut import), 0);
    assert_eq!(call(FN_PRESENT, &mut Buffer { id: 7 }), 0);

    // Vsync: a steady stream at the display's period.
    assert_eq!(call(FN_SET_VSYNC, &mut SetVsync { enabled: 1 }), 0);
    let mut file = std::fs::File::from(events);
    let mut stamps = Vec::new();
    while stamps.len() < 30 {
        let mut e = Event::default();
        // SAFETY: `Event` is plain old data.
        let buf = unsafe {
            std::slice::from_raw_parts_mut((&mut e as *mut Event).cast(), size_of::<Event>())
        };
        file.read_exact(buf).unwrap();
        assert_eq!(e.kind, event::VSYNC);
        stamps.push(e.timestamp_ns);
    }
    let period = info.vsync_period_ns as i64;
    for w in stamps.windows(2) {
        let d = w[1] - w[0];
        // Whole periods (a busy test host may miss callbacks), each within
        // 1% of the model's period.
        let n = (d + period / 2) / period;
        assert!(
            n >= 1 && (d - n * period).abs() < period / 100,
            "interval {d} ns, period {period}"
        );
    }

    // The presented buffer, read back by the server.
    // SAFETY: a signal to the server we started.
    unsafe { libc::kill(server.0.id() as i32, libc::SIGUSR1) };
    let bmp = wait_for(&capture);
    for (x, y) in [(0, 0), (W - 1, 0), (5, H - 1), (W / 2, H / 2)] {
        let at = 54 + ((y * W + x) * 4) as usize;
        let [r, g, b, a] = pixel(x, y);
        assert_eq!(&bmp[at..at + 4], &[b, g, r, a], "pixel {x},{y}");
    }
    let _ = std::fs::remove_file(&socket);
    let _ = std::fs::remove_dir_all(&dir);
}
