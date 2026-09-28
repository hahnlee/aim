//! Zygote's pattern on the host: load the driver and get the display, then
//! fork, and compile and draw in the child (`docs/gles-driver.md`,
//! "Displays and fork"). A guest fork is a freshly spawned process that
//! takes over the module's state ([`fork_state`]), as the syscall layer's
//! fork does; this test spawns itself the same way. The child compiles a
//! shader unique to the run, so Metal's shader cache cannot stand in for
//! the compiler service. Skipped when the ANGLE build is absent.
//!
//! (A Darwin `fork()` child cannot do this: launchd refuses its lookup of
//! `MTLCompilerService`, and linking fails with "Unable to reach
//! MTLCompilerService".)

use std::ffi::CStr;

use aim_host_gpu::{
    MODULE, fork_state, function, restore_fork_state, set_library_dir, table_identity,
};
use aim_hostcall::gpu::{FN_INIT, Init};

/// Call a forwarded entry point with its register image.
fn x(name: &CStr, args: &[u64]) -> u64 {
    let id = function(name).unwrap_or_else(|| panic!("{name:?} not forwarded"));
    // SAFETY: the values are this entry point's register image.
    unsafe { (MODULE.call)(id, args.as_ptr() as u64, (args.len() * 8) as u64) as u64 }
}

fn metal_display() -> u64 {
    let attribs = [0x3203i64, 0x3489, 0x3038];
    x(
        c"eglGetPlatformDisplay",
        &[0x3202, 0, attribs.as_ptr() as u64],
    )
}

fn initialize(dpy: u64) -> bool {
    let (mut major, mut minor) = (0i32, 0i32);
    x(
        c"eglInitialize",
        &[
            dpy,
            &mut major as *mut _ as u64,
            &mut minor as *mut _ as u64,
        ],
    ) == 1
}

/// Draw a green triangle over red with a compiled program (which needs
/// Metal's compiler service) and read one pixel back; initialize the
/// display first unless `initialized`.
fn draw(dpy: u64, initialized: bool) -> Result<(), String> {
    if !initialized && !initialize(dpy) {
        return Err(format!("eglInitialize: {:#x}", x(c"eglGetError", &[])));
    }
    let config_attribs = [
        0x3024, 8, 0x3023, 8, 0x3022, 8, 0x3021, 8, 0x3040, 0x40, 0x3033, 1, 0x3038i32,
    ];
    let (mut config, mut n) = (0u64, 0i32);
    x(
        c"eglChooseConfig",
        &[
            dpy,
            config_attribs.as_ptr() as u64,
            &mut config as *mut _ as u64,
            1,
            &mut n as *mut _ as u64,
        ],
    );
    let context_attribs = [0x3098, 3, 0x3038i32];
    let ctx = x(
        c"eglCreateContext",
        &[dpy, config, 0, context_attribs.as_ptr() as u64],
    );
    let surface_attribs = [0x3057, 16, 0x3056, 16, 0x3038i32];
    let surface = x(
        c"eglCreatePbufferSurface",
        &[dpy, config, surface_attribs.as_ptr() as u64],
    );
    if x(c"eglMakeCurrent", &[dpy, surface, surface, ctx]) != 1 {
        return Err("eglMakeCurrent".into());
    }
    assert_eq!(x(c"eglGetCurrentDisplay", &[]), dpy, "the guest's handle");

    let vs = c"#version 300 es
        void main() {
            vec2 p[3] = vec2[3](vec2(-1, -1), vec2(3, -1), vec2(-1, 3));
            gl_Position = vec4(p[gl_VertexID], 0, 1);
        }";
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
        ^ u64::from(std::process::id()) << 40;
    let fs = std::ffi::CString::new(format!(
        "#version 300 es
        precision highp float;
        out vec4 color;
        uniform float u{nonce};
        void main() {{ color = vec4(0, 1, 0, 1) + vec4(u{nonce} * {}.0); }}",
        nonce % 100_000
    ))
    .unwrap();
    let fs = fs.as_c_str();
    let program = x(c"glCreateProgram", &[]);
    for (kind, source) in [(0x8b31u64, vs), (0x8b30, fs)] {
        let shader = x(c"glCreateShader", &[kind]);
        let sources = [source.as_ptr()];
        x(c"glShaderSource", &[shader, 1, sources.as_ptr() as u64, 0]);
        x(c"glCompileShader", &[shader]);
        x(c"glAttachShader", &[program, shader]);
    }
    x(c"glLinkProgram", &[program]);
    let mut linked = 0i32;
    x(
        c"glGetProgramiv",
        &[program, 0x8b82, &mut linked as *mut _ as u64],
    );
    if linked != 1 {
        let mut log = [0u8; 2048];
        let mut n = 0i32;
        x(
            c"glGetProgramInfoLog",
            &[
                program,
                2048,
                &mut n as *mut _ as u64,
                log.as_mut_ptr() as u64,
            ],
        );
        return Err(format!(
            "glLinkProgram failed: {}",
            String::from_utf8_lossy(&log[..n as usize])
        ));
    }
    let red = [1f32, 0.0, 0.0, 1.0].map(|v| v.to_bits() as u64);
    // SAFETY: four d-register values.
    unsafe { (MODULE.call)(function(c"glClearColor").unwrap(), red.as_ptr() as u64, 32) };
    x(c"glClear", &[0x4000]);
    x(c"glUseProgram", &[program]);
    x(c"glDrawArrays", &[4, 0, 3]);
    let mut pixel = 0u32;
    x(
        c"glReadPixels",
        &[8, 8, 1, 1, 0x1908, 0x1401, &mut pixel as *mut _ as u64],
    );
    x(c"eglMakeCurrent", &[dpy, 0, 0, 0]);
    if pixel != 0xff00_ff00 {
        return Err(format!("pixel {pixel:#010x}"));
    }
    Ok(())
}

/// The child: take over the parent's state (passed as hex on the command
/// line) and draw on its display, without loading the driver (the guest's
/// driver would not ask again).
fn child(state: &str, dpy: u64, initialized: bool) -> i32 {
    let state: Vec<u8> = (0..state.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&state[i..i + 2], 16).unwrap())
        .collect();
    set_library_dir(&aim_paths::angle());
    restore_fork_state(&state);
    match draw(dpy, initialized) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("child: {e}");
            1
        }
    }
}

/// Fork the way the syscall layer does: spawn this program with the state.
fn fork(dpy: u64, initialized: bool) -> i32 {
    let state: String = fork_state().iter().map(|b| format!("{b:02x}")).collect();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["child", &state, &dpy.to_string(), &initialized.to_string()])
        .status()
        .unwrap();
    status.code().unwrap_or(-1)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 5 && args[1] == "child" {
        std::process::exit(child(&args[2], args[3].parse().unwrap(), args[4] == "true"));
    }
    if aim_paths::input(aim_paths::angle().join("libGLESv2.dylib"), "angle").is_none() {
        return;
    }
    set_library_dir(&aim_paths::angle());
    let (hash, len) = table_identity();
    let mut bits = vec![0u64; (len as usize).div_ceil(64)];
    let mut init = Init {
        table_hash: hash,
        table_len: len,
        resolved: bits.as_mut_ptr() as u64,
        resolved_words: bits.len() as u64,
    };
    // SAFETY: FN_INIT's argument block.
    let r = unsafe {
        (MODULE.call)(
            FN_INIT,
            &mut init as *mut Init as u64,
            size_of::<Init>() as u64,
        )
    };
    assert_eq!(r, 0);

    // Zygote: load the driver and get the display, then fork.
    let dpy = metal_display();
    assert_ne!(dpy, 0);
    assert_eq!(
        fork(dpy, false),
        0,
        "the child renders with a compiled program"
    );

    // A display the parent initialized and used is made again in the
    // child, initialized, for the guest's EGL that will not ask again.
    draw(dpy, false).expect("the parent renders");
    assert_eq!(
        fork(dpy, true),
        0,
        "the child of an initialized display renders"
    );
}
