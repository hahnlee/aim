//! `GL_OES_EGL_image_external` (and `_essl3`), which ANGLE's Metal backend
//! does not offer, emulated over 2D textures.
//!
//! Android samples graphics buffers through external textures: Skia (in
//! SurfaceFlinger's RenderEngine and HWUI) binds every buffer it only reads
//! to `GL_TEXTURE_EXTERNAL_OES`. Our buffers are ordinary linear textures,
//! so an external texture is a 2D texture, as in ANGLE's Vulkan backend.
//!
//! A texture unit has separate 2D and external binding points, and callers
//! (Skia) cache both, so the external binding of unit `u` lives in the 2D
//! binding of the hidden unit `u + K`, where `K` is half the host's units;
//! the guest is told it has `K` units. Shaders get `sampler2D` for
//! `samplerExternalOES`, and a program's external samplers are pointed at
//! their hidden units (`glUniform1i` adds `K`). A sampler object bound to
//! `u` is bound to `u + K` too, and a new external texture gets an external
//! texture's initial filtering and wrapping.

use std::collections::HashMap;
use std::ffi::{CStr, c_char};
use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::thunks::host;
use crate::types::*;

const GL_EXTENSIONS: GLenum = 0x1f03;
const GL_NUM_EXTENSIONS: GLenum = 0x821d;
const GL_TEXTURE_2D: GLenum = 0x0de1;
const GL_TEXTURE_EXTERNAL_OES: GLenum = 0x8d65;
const GL_TEXTURE_BINDING_2D: GLenum = 0x8069;
const GL_TEXTURE_BINDING_EXTERNAL_OES: GLenum = 0x8d67;
const GL_REQUIRED_TEXTURE_IMAGE_UNITS_OES: GLenum = 0x8d68;
const GL_ACTIVE_TEXTURE: GLenum = 0x84e0;
const GL_CURRENT_PROGRAM: GLenum = 0x8b8d;
const GL_MAX_TEXTURE_IMAGE_UNITS: GLenum = 0x8872;
const GL_MAX_VERTEX_TEXTURE_IMAGE_UNITS: GLenum = 0x8b4c;
const GL_MAX_COMBINED_TEXTURE_IMAGE_UNITS: GLenum = 0x8b4d;
const GL_LINK_STATUS: GLenum = 0x8b82;
const GL_TEXTURE_MIN_FILTER: GLenum = 0x2801;
const GL_TEXTURE_WRAP_S: GLenum = 0x2802;
const GL_TEXTURE_WRAP_T: GLenum = 0x2803;
const GL_LINEAR: GLint = 0x2601;
const GL_CLAMP_TO_EDGE: GLint = 0x812f;

/// The initial state of an external texture that differs from a 2D one's
/// (`OES_EGL_image_external`, "Changes to Section 3.7.14").
const EXTERNAL_DEFAULTS: [(GLenum, GLint); 3] = [
    (GL_TEXTURE_MIN_FILTER, GL_LINEAR),
    (GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE),
    (GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE),
];

const OURS: [&CStr; 2] = [
    c"GL_OES_EGL_image_external",
    c"GL_OES_EGL_image_external_essl3",
];

/// `K`: the first hidden unit.
fn hidden_base() -> GLint {
    static BASE: OnceLock<GLint> = OnceLock::new();
    *BASE.get_or_init(|| {
        let mut n = 0;
        // SAFETY: forwarded query into a local.
        unsafe { host::glGetIntegerv(GL_MAX_COMBINED_TEXTURE_IMAGE_UNITS, &mut n) };
        n / 2
    })
}

/// Per context: each shader's external sampler names, and each program's
/// external sampler locations.
#[derive(Default)]
struct Context {
    shaders: HashMap<GLuint, Vec<String>>,
    programs: HashMap<GLuint, Vec<GLint>>,
}

fn contexts() -> MutexGuard<'static, HashMap<usize, Context>> {
    static CONTEXTS: OnceLock<Mutex<HashMap<usize, Context>>> = OnceLock::new();
    CONTEXTS.get_or_init(Default::default).lock().unwrap()
}

fn current() -> usize {
    // SAFETY: forwarded.
    unsafe { host::eglGetCurrentContext() as usize }
}

/// Run `f` with the hidden unit of the active unit selected.
fn on_hidden_unit<T>(f: impl FnOnce() -> T) -> T {
    let mut active = 0;
    // SAFETY: forwarded GL calls on the current context.
    unsafe {
        host::glGetIntegerv(GL_ACTIVE_TEXTURE, &mut active);
        host::glActiveTexture(active as GLenum + hidden_base() as GLenum);
        let r = f();
        host::glActiveTexture(active as GLenum);
        r
    }
}

/// # Safety
/// GL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glGetString(name: GLenum) -> *const GLubyte {
    // SAFETY: forwarded.
    let s = unsafe { host::glGetString(name) };
    if name != GL_EXTENSIONS || s.is_null() {
        return s;
    }
    static MERGED: OnceLock<std::ffi::CString> = OnceLock::new();
    MERGED
        .get_or_init(|| {
            // SAFETY: the host's NUL-terminated extension string.
            let mut all = unsafe { CStr::from_ptr(s.cast()) }.to_bytes().to_vec();
            for e in OURS {
                all.push(b' ');
                all.extend_from_slice(e.to_bytes());
            }
            std::ffi::CString::new(all).unwrap()
        })
        .as_ptr()
        .cast()
}

/// # Safety
/// GL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glGetStringi(name: GLenum, index: GLuint) -> *const GLubyte {
    if name == GL_EXTENSIONS {
        let mut n = 0;
        // SAFETY: forwarded query into a local.
        unsafe { host::glGetIntegerv(GL_NUM_EXTENSIONS, &mut n) };
        if let Some(e) = (index as usize)
            .checked_sub(n as usize)
            .and_then(|i| OURS.get(i))
        {
            return e.as_ptr().cast();
        }
    }
    // SAFETY: forwarded.
    unsafe { host::glGetStringi(name, index) }
}

/// # Safety
/// GL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glGetIntegerv(pname: GLenum, data: *mut GLint) {
    // SAFETY (all arms): forwarded; `data` is the caller's.
    unsafe {
        match pname {
            GL_TEXTURE_BINDING_EXTERNAL_OES => {
                on_hidden_unit(|| host::glGetIntegerv(GL_TEXTURE_BINDING_2D, data))
            }
            GL_REQUIRED_TEXTURE_IMAGE_UNITS_OES => *data = 1,
            _ => {
                host::glGetIntegerv(pname, data);
                match pname {
                    GL_NUM_EXTENSIONS => *data += OURS.len() as GLint,
                    GL_MAX_TEXTURE_IMAGE_UNITS
                    | GL_MAX_VERTEX_TEXTURE_IMAGE_UNITS
                    | GL_MAX_COMBINED_TEXTURE_IMAGE_UNITS => *data = (*data).min(hidden_base()),
                    _ => {}
                }
            }
        }
    }
}

/// # Safety
/// GL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glBindTexture(target: GLenum, texture: GLuint) {
    // SAFETY: forwarded.
    unsafe {
        if target != GL_TEXTURE_EXTERNAL_OES {
            return host::glBindTexture(target, texture);
        }
        on_hidden_unit(|| {
            // A name not yet bound is not yet a texture: this bind creates it.
            let new = texture != 0 && host::glIsTexture(texture) == 0;
            host::glBindTexture(GL_TEXTURE_2D, texture);
            if new {
                // An external texture starts with linear filtering and
                // clamped wrapping; a 2D one would start with a mipmap
                // filter, which leaves a one-level texture incomplete.
                for (pname, param) in EXTERNAL_DEFAULTS {
                    host::glTexParameteri(GL_TEXTURE_2D, pname, param);
                }
            }
        })
    }
}

/// # Safety
/// GL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glBindSampler(unit: GLuint, sampler: GLuint) {
    // A sampler object applies to every target of its unit, so it also
    // applies to the unit's external binding on the hidden unit.
    // SAFETY: forwarded; the guest's units are below `K`.
    unsafe {
        host::glBindSampler(unit, sampler);
        if (unit as GLint) < hidden_base() {
            host::glBindSampler(unit + hidden_base() as GLuint, sampler);
        }
    }
}

/// # Safety
/// GL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glEGLImageTargetTexture2DOES(target: GLenum, image: GLeglImageOES) {
    // SAFETY: forwarded.
    unsafe {
        if target == GL_TEXTURE_EXTERNAL_OES {
            on_hidden_unit(|| host::glEGLImageTargetTexture2DOES(GL_TEXTURE_2D, image))
        } else {
            host::glEGLImageTargetTexture2DOES(target, image)
        }
    }
}

macro_rules! tex_parameter {
    ($name:ident, $t:ty) => {
        /// # Safety
        /// GL's contract.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(target: GLenum, pname: GLenum, param: $t) {
            // SAFETY: forwarded.
            unsafe {
                if target == GL_TEXTURE_EXTERNAL_OES {
                    on_hidden_unit(|| host::$name(GL_TEXTURE_2D, pname, param))
                } else {
                    host::$name(target, pname, param)
                }
            }
        }
    };
}

tex_parameter!(glTexParameteri, GLint);
tex_parameter!(glTexParameterf, GLfloat);
tex_parameter!(glTexParameteriv, *const GLint);
tex_parameter!(glTexParameterfv, *const GLfloat);
tex_parameter!(glGetTexParameteriv, *mut GLint);
tex_parameter!(glGetTexParameterfv, *mut GLfloat);

/// `source` with `samplerExternalOES` as `sampler2D` and the extension's
/// `#extension` lines dropped, and the names of the samplers it declares.
fn rewrite(source: &str) -> (String, Vec<String>) {
    let mut names = Vec::new();
    let mut out = String::with_capacity(source.len());
    for line in source.split_inclusive('\n') {
        let t = line.trim_start();
        if t.starts_with("#extension") && t.contains("GL_OES_EGL_image_external") {
            // Keep line numbers.
            out.push('\n');
            continue;
        }
        let mut rest = line;
        while let Some(at) = rest.find("samplerExternalOES") {
            out.push_str(&rest[..at]);
            out.push_str("sampler2D");
            rest = &rest[at + "samplerExternalOES".len()..];
            let name: String = rest
                .trim_start()
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                names.push(name);
            }
        }
        out.push_str(rest);
    }
    (out, names)
}

/// # Safety
/// GL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glShaderSource(
    shader: GLuint,
    count: GLsizei,
    string: *const *mut GLchar,
    length: *const GLint,
) {
    let mut source = Vec::new();
    for i in 0..count.max(0) as usize {
        // SAFETY: `count` strings, NUL-terminated where `length` has no
        // non-negative entry for them.
        unsafe {
            let s = *string.add(i);
            let n = if length.is_null() || *length.add(i) < 0 {
                CStr::from_ptr(s as *const c_char).to_bytes().len()
            } else {
                *length.add(i) as usize
            };
            source.extend_from_slice(std::slice::from_raw_parts(s as *const u8, n));
        }
    }
    let text = String::from_utf8_lossy(&source);
    if !text.contains("samplerExternalOES") {
        contexts()
            .entry(current())
            .or_default()
            .shaders
            .remove(&shader);
        // SAFETY: forwarded.
        return unsafe { host::glShaderSource(shader, count, string, length) };
    }
    let (text, names) = rewrite(&text);
    contexts()
        .entry(current())
        .or_default()
        .shaders
        .insert(shader, names);
    let len = text.len() as GLint;
    let ptr = text.as_ptr() as *mut GLchar;
    // SAFETY: forwarded with one string of explicit length.
    unsafe { host::glShaderSource(shader, 1, &ptr, &len) }
}

/// # Safety
/// GL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glLinkProgram(program: GLuint) {
    // SAFETY: forwarded.
    unsafe { host::glLinkProgram(program) };
    let context = current();
    let mut all = contexts();
    let Some(ctx) = all.get_mut(&context) else {
        return;
    };
    ctx.programs.remove(&program);
    let mut shaders = [0; 8];
    let mut count = 0;
    let mut linked = 0;
    // SAFETY: forwarded queries into locals.
    unsafe {
        host::glGetProgramiv(program, GL_LINK_STATUS, &mut linked);
        host::glGetAttachedShaders(program, 8, &mut count, shaders.as_mut_ptr());
    }
    let mut locations = Vec::new();
    for s in &shaders[..count.clamp(0, 8) as usize] {
        for name in ctx.shaders.get(s).into_iter().flatten() {
            let c = std::ffi::CString::new(name.as_str()).unwrap();
            // SAFETY: forwarded with a NUL-terminated name.
            let loc = unsafe { host::glGetUniformLocation(program, c.as_ptr().cast()) };
            if loc >= 0 {
                locations.push(loc);
            }
        }
    }
    if linked == 0 || locations.is_empty() {
        return;
    }
    // Samplers start at unit 0, which for an external one is hidden unit K.
    let mut previous = 0;
    // SAFETY: forwarded GL calls; the previous program is restored.
    unsafe {
        host::glGetIntegerv(GL_CURRENT_PROGRAM, &mut previous);
        host::glUseProgram(program);
        for &loc in &locations {
            host::glUniform1i(loc, hidden_base());
        }
        host::glUseProgram(previous as GLuint);
    }
    ctx.programs.insert(program, locations);
}

/// Whether `location` of the current program is an external sampler.
fn is_external(location: GLint) -> bool {
    let all = contexts();
    let Some(ctx) = all.get(&current()) else {
        return false;
    };
    if ctx.programs.is_empty() {
        return false;
    }
    let mut program = 0;
    // SAFETY: forwarded query into a local.
    unsafe { host::glGetIntegerv(GL_CURRENT_PROGRAM, &mut program) };
    ctx.programs
        .get(&(program as GLuint))
        .is_some_and(|l| l.contains(&location))
}

/// # Safety
/// GL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glUniform1i(location: GLint, v: GLint) {
    let v = if is_external(location) {
        v + hidden_base()
    } else {
        v
    };
    // SAFETY: forwarded.
    unsafe { host::glUniform1i(location, v) }
}

/// # Safety
/// GL's contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glUniform1iv(location: GLint, count: GLsizei, value: *const GLint) {
    if count == 1 && !value.is_null() && is_external(location) {
        // SAFETY: one value, as `count` says.
        let v = unsafe { *value } + hidden_base();
        // SAFETY: forwarded.
        return unsafe { host::glUniform1i(location, v) };
    }
    // SAFETY: forwarded.
    unsafe { host::glUniform1iv(location, count, value) }
}
