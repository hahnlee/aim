//! The display server end to end, from the host side of the module: a
//! buffer in a file (as a memfd is on the host) is imported, presented and
//! captured back, vsync events arrive at the display's rate while enabled
//! and stop while disabled, present fences signal on the display's vsync
//! timeline, and a frame of layers (window mode's present) is shown once
//! all its layers' content is ready.
//!
//! Opens a small window: run it in a logged-in session.

use std::io::Read;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use aim_host_display::{MODULE, set_server};
use aim_hostcall::display::{
    Connect, Event, FN_CONNECT, FN_IMPORT, FN_LAYERS, FN_PRESENT, FN_SET_VSYNC, Import, Layer,
    Layers, Present, SetVsync, event, layer, mode,
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

/// The next vsync event, or `None` after `timeout_ms`.
fn next_vsync(events: &OwnedFd, timeout_ms: i32) -> Option<Event> {
    let mut pfd = libc::pollfd {
        fd: events.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: polls one local pollfd.
    if unsafe { libc::poll(&mut pfd, 1, timeout_ms) } != 1 {
        return None;
    }
    let mut e = Event::default();
    // SAFETY: reads one plain-old-data record from our socket.
    let n = unsafe {
        libc::read(
            events.as_raw_fd(),
            (&mut e as *mut Event).cast(),
            size_of::<Event>(),
        )
    };
    assert_eq!(n, size_of::<Event>() as isize);
    assert_eq!(e.kind, event::VSYNC);
    Some(e)
}

/// Present buffer 7 with `acquire` (-1 for none); its present fence.
fn present(acquire: i32) -> OwnedFd {
    let mut present = Present {
        id: 7,
        acquire,
        present: -1,
    };
    assert_eq!(call(FN_PRESENT, &mut present), 0);
    // SAFETY: the module returned a new fd for us.
    unsafe { OwnedFd::from_raw_fd(present.present) }
}

fn shown_at(fence: &OwnedFd) -> i64 {
    assert!(aim_sync_file::wait(fence.as_fd(), 5000), "present fence");
    let aim_sync_file::State::Signaled {
        timestamp_ns,
        status: 1,
    } = aim_sync_file::state(fence.as_fd())
    else {
        panic!("{:?}", aim_sync_file::state(fence.as_fd()));
    };
    timestamp_ns
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
    let mut server = Command::new(env!("CARGO_BIN_EXE_aim-display"))
        .args(["--size", &format!("{W}x{H}"), "--title", "aim-display test"])
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
    // The server shows the buffer once its acquire fence signals, and the
    // present fence signals once it is on screen.
    let (acquire, content) = aim_sync_file::pair().unwrap();
    let shown = present(acquire.as_raw_fd());
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        !aim_sync_file::wait(shown.as_fd(), 0),
        "shown before its content"
    );
    let ready = aim_sync_file::monotonic_ns();
    content.signal(1);
    let timestamp_ns = shown_at(&shown);
    assert!(
        timestamp_ns > ready,
        "shown at {timestamp_ns}, ready at {ready}"
    );

    // A frame of layers: the buffer, then a flipped quarter of it over a
    // color, each buffer layer with its own acquire fence. It is shown
    // once both have signaled.
    assert_eq!(info.mode, mode::DEVICE);
    let (first, first_content) = aim_sync_file::pair().unwrap();
    let (second, second_content) = aim_sync_file::pair().unwrap();
    let whole = [0, 0, W as i32, H as i32];
    let layers = [
        Layer {
            id: 1,
            buffer: 7,
            kind: layer::BUFFER,
            frame: whole,
            crop: [0.0, 0.0, W as f32, H as f32],
            alpha: 1.0,
            blend: 1,
            acquire: first.as_raw_fd(),
            ..Default::default()
        },
        Layer {
            id: 2,
            kind: layer::COLOR,
            frame: [0, 0, 16, 16],
            color: [1.0, 0.0, 0.0, 1.0],
            alpha: 0.5,
            blend: 2,
            acquire: -1,
            ..Default::default()
        },
        Layer {
            id: 3,
            buffer: 7,
            kind: layer::BUFFER,
            transform: 1,
            frame: [8, 8, 40, 32],
            crop: [0.0, 0.0, 32.0, 24.0],
            alpha: 1.0,
            blend: 2,
            visible_count: 1,
            acquire: second.as_raw_fd(),
            ..Default::default()
        },
    ];
    let rects = [[8, 8, 40, 32]];
    let mut frame = Layers {
        layers: layers.as_ptr() as u64,
        count: layers.len() as u32,
        rect_count: rects.len() as u32,
        rects: rects.as_ptr() as u64,
        client_target: 0,
        client_acquire: -1,
        present: -1,
    };
    assert_eq!(call(FN_LAYERS, &mut frame), 0);
    // SAFETY: the module returned a new fd for us.
    let shown = unsafe { OwnedFd::from_raw_fd(frame.present) };
    first_content.signal(1);
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        !aim_sync_file::wait(shown.as_fd(), 0),
        "shown before all its layers' content"
    );
    let ready = aim_sync_file::monotonic_ns();
    second_content.signal(1);
    let timestamp_ns = shown_at(&shown);
    assert!(
        timestamp_ns > ready,
        "shown at {timestamp_ns}, ready at {ready}"
    );

    // Vsync: a steady stream at the display's period.
    assert_eq!(call(FN_SET_VSYNC, &mut SetVsync { enabled: 1 }), 0);
    let mut stamps = Vec::new();
    while stamps.len() < 30 {
        stamps.push(next_vsync(&events, 1000).expect("vsync").timestamp_ns);
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

    // A present fence signals at a vsync, which SurfaceFlinger's vsync
    // model relies on.
    let at = shown_at(&present(-1));
    let phase = (at - stamps[stamps.len() - 1]).rem_euclid(period);
    assert!(
        phase.min(period - phase) < period / 20,
        "shown {phase} ns after a vsync, period {period}"
    );

    // Disabled, no more vsyncs arrive (after those already sent).
    assert_eq!(call(FN_SET_VSYNC, &mut SetVsync { enabled: 0 }), 0);
    for _ in 0..10 {
        if next_vsync(&events, 50).is_none() {
            break;
        }
    }
    assert!(next_vsync(&events, 300).is_none(), "vsync while disabled");

    // The presented buffer, read back by the server while vsync is off.
    // SAFETY: a signal to the server we started.
    unsafe { libc::kill(server.0.id() as i32, libc::SIGUSR1) };
    let bmp = wait_for(&capture);
    for (x, y) in [(0, 0), (W - 1, 0), (5, H - 1), (W / 2, H / 2)] {
        let at = 54 + ((y * W + x) * 4) as usize;
        let [r, g, b, a] = pixel(x, y);
        assert_eq!(&bmp[at..at + 4], &[b, g, r, a], "pixel {x},{y}");
    }

    // Enabled again, vsync resumes within a few periods.
    let enabled = Instant::now();
    assert_eq!(call(FN_SET_VSYNC, &mut SetVsync { enabled: 1 }), 0);
    assert!(
        next_vsync(&events, 200).is_some(),
        "no vsync after enabling"
    );
    eprintln!("first vsync {:?} after enabling", enabled.elapsed());
    let _ = std::fs::remove_file(&socket);
    let _ = std::fs::remove_dir_all(&dir);
}
