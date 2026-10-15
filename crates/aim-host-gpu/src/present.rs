//! `eglSwapBuffers` on a window surface: the guest renders into an ANGLE
//! pbuffer (the surface's default framebuffer); at swap its color buffer is
//! blitted into the buffer being queued. The destination framebuffer has
//! `GL_MESA_framebuffer_flip_y`, so the buffer's row 0 is the top row, as
//! Android expects of window buffers. The buffer is queued with a fence
//! for the blit ([`crate::fence`]); nothing waits for the GPU here.
//!
//! The framebuffer over each buffer is made once per context and kept
//! ([`Target`]) until the buffer's image or the context goes away.

use std::ffi::CStr;
use std::sync::{Mutex, OnceLock};

use aim_hostcall::gpu::Present;

use crate::{EINVAL, fence, resolved};

const GL_TEXTURE_2D: u32 = 0x0de1;
const GL_TEXTURE_BINDING_2D: u32 = 0x8069;
const GL_SCISSOR_TEST: u32 = 0x0c11;
const GL_READ_FRAMEBUFFER: u32 = 0x8ca8;
const GL_DRAW_FRAMEBUFFER: u32 = 0x8ca9;
const GL_DRAW_FRAMEBUFFER_BINDING: u32 = 0x8ca6;
const GL_READ_FRAMEBUFFER_BINDING: u32 = 0x8caa;
const GL_COLOR_ATTACHMENT0: u32 = 0x8ce0;
const GL_FRAMEBUFFER_FLIP_Y_MESA: u32 = 0x8bbb;
const GL_COLOR_BUFFER_BIT: u32 = 0x4000;
const GL_NEAREST: u32 = 0x2600;
const GL_LINEAR: u32 = 0x2601;
const EGL_CONTEXT_CLIENT_VERSION: i32 = 0x3098;

type Blit = unsafe extern "C" fn(i32, i32, i32, i32, i32, i32, i32, i32, u32, u32);

/// The ANGLE entry points a present uses.
struct Gl {
    get_current_display: unsafe extern "C" fn() -> usize,
    get_current_context: unsafe extern "C" fn() -> usize,
    query_context: unsafe extern "C" fn(usize, usize, i32, *mut i32) -> u32,
    get_integerv: unsafe extern "C" fn(u32, *mut i32),
    is_enabled: unsafe extern "C" fn(u32) -> u8,
    enable: unsafe extern "C" fn(u32),
    disable: unsafe extern "C" fn(u32),
    gen_textures: unsafe extern "C" fn(i32, *mut u32),
    delete_textures: unsafe extern "C" fn(i32, *const u32),
    bind_texture: unsafe extern "C" fn(u32, u32),
    image_target_texture: unsafe extern "C" fn(u32, usize),
    gen_framebuffers: unsafe extern "C" fn(i32, *mut u32),
    delete_framebuffers: unsafe extern "C" fn(i32, *const u32),
    bind_framebuffer: unsafe extern "C" fn(u32, u32),
    framebuffer_texture: unsafe extern "C" fn(u32, u32, u32, u32, i32),
    framebuffer_parameter: unsafe extern "C" fn(u32, u32, i32),
    blit: Blit,
    blit_angle: Blit,
}

/// ANGLE's entry point `name` as the function pointer type `F`.
///
/// # Safety
/// `F` must be the C signature of that entry point.
pub(crate) unsafe fn entry<F>(name: &CStr) -> Option<F> {
    let address = resolved(name);
    // SAFETY: a non-null function address, as the caller's type.
    (address != 0).then(|| unsafe { std::mem::transmute_copy(&address) })
}

fn gl() -> Option<&'static Gl> {
    static GL: OnceLock<Option<Gl>> = OnceLock::new();
    GL.get_or_init(|| {
        // SAFETY: each field's type is the C signature of that entry point.
        unsafe {
            Some(Gl {
                get_current_display: entry(c"eglGetCurrentDisplay")?,
                get_current_context: entry(c"eglGetCurrentContext")?,
                query_context: entry(c"eglQueryContext")?,
                get_integerv: entry(c"glGetIntegerv")?,
                is_enabled: entry(c"glIsEnabled")?,
                enable: entry(c"glEnable")?,
                disable: entry(c"glDisable")?,
                gen_textures: entry(c"glGenTextures")?,
                delete_textures: entry(c"glDeleteTextures")?,
                bind_texture: entry(c"glBindTexture")?,
                image_target_texture: entry(c"glEGLImageTargetTexture2DOES")?,
                gen_framebuffers: entry(c"glGenFramebuffers")?,
                delete_framebuffers: entry(c"glDeleteFramebuffers")?,
                bind_framebuffer: entry(c"glBindFramebuffer")?,
                framebuffer_texture: entry(c"glFramebufferTexture2D")?,
                framebuffer_parameter: entry(c"glFramebufferParameteriMESA")?,
                blit: entry(c"glBlitFramebuffer")?,
                blit_angle: entry(c"glBlitFramebufferANGLE")?,
            })
        }
    })
    .as_ref()
}

/// A context's framebuffer over a buffer's image.
struct Target {
    context: usize,
    image: usize,
    texture: u32,
    fbo: u32,
    /// The context's client version.
    version: i32,
}

/// Framebuffers in use, and those whose image was destroyed, which their
/// context deletes at its next present.
static TARGETS: Mutex<(Vec<Target>, Vec<Target>)> = Mutex::new((Vec::new(), Vec::new()));

/// ANGLE destroyed `image` (`eglDestroyImage`).
pub fn image_destroyed(image: usize) {
    let mut t = TARGETS.lock().unwrap();
    let (live, dead) = &mut *t;
    let (gone, keep): (Vec<Target>, Vec<Target>) = live.drain(..).partition(|x| x.image == image);
    *live = keep;
    dead.extend(gone);
}

/// ANGLE destroyed `context`, and with it its framebuffers.
pub fn context_destroyed(context: usize) {
    let mut t = TARGETS.lock().unwrap();
    t.0.retain(|x| x.context != context);
    t.1.retain(|x| x.context != context);
}

/// The current context's framebuffer over `image`, made on first use.
///
/// # Safety
/// ANGLE's GL on this thread's current context `context`.
unsafe fn target(g: &Gl, display: usize, context: usize, image: usize) -> (u32, i32) {
    let mut t = TARGETS.lock().unwrap();
    let (live, dead) = &mut *t;
    let (mine, others): (Vec<Target>, Vec<Target>) =
        dead.drain(..).partition(|x| x.context == context);
    *dead = others;
    for x in mine {
        // SAFETY: objects of the current context.
        unsafe {
            (g.delete_framebuffers)(1, &x.fbo);
            (g.delete_textures)(1, &x.texture);
        }
    }
    if let Some(x) = live
        .iter()
        .find(|x| x.context == context && x.image == image)
    {
        return (x.fbo, x.version);
    }
    let (mut texture, mut fbo, mut version, mut bound) = (0, 0, 0, 0);
    // SAFETY: caller contract; the texture binding is restored.
    unsafe {
        (g.query_context)(display, context, EGL_CONTEXT_CLIENT_VERSION, &mut version);
        (g.get_integerv)(GL_TEXTURE_BINDING_2D, &mut bound);
        (g.gen_textures)(1, &mut texture);
        (g.bind_texture)(GL_TEXTURE_2D, texture);
        (g.image_target_texture)(GL_TEXTURE_2D, image);
        (g.bind_texture)(GL_TEXTURE_2D, bound as u32);
        (g.gen_framebuffers)(1, &mut fbo);
        (g.bind_framebuffer)(GL_DRAW_FRAMEBUFFER, fbo);
        (g.framebuffer_texture)(
            GL_DRAW_FRAMEBUFFER,
            GL_COLOR_ATTACHMENT0,
            GL_TEXTURE_2D,
            texture,
            0,
        );
        (g.framebuffer_parameter)(GL_DRAW_FRAMEBUFFER, GL_FRAMEBUFFER_FLIP_Y_MESA, 1);
    }
    live.push(Target {
        context,
        image,
        texture,
        fbo,
        version,
    });
    (fbo, version)
}

pub fn present(p: &mut Present) -> i64 {
    let Some(g) = gl() else {
        return EINVAL;
    };
    if p.image == 0 || p.src_width == 0 || p.src_height == 0 {
        return EINVAL;
    }
    // SAFETY: ANGLE's GL on this thread's current context; every binding
    // changed here is restored.
    unsafe {
        let context = (g.get_current_context)();
        if context == 0 {
            return EINVAL;
        }
        let display = (g.get_current_display)();
        let (mut draw, mut read) = (0, 0);
        (g.get_integerv)(GL_DRAW_FRAMEBUFFER_BINDING, &mut draw);
        (g.get_integerv)(GL_READ_FRAMEBUFFER_BINDING, &mut read);
        let scissor = (g.is_enabled)(GL_SCISSOR_TEST) != 0;

        let (fbo, version) = target(g, display, context, p.image as usize);
        (g.bind_framebuffer)(GL_DRAW_FRAMEBUFFER, fbo);
        (g.bind_framebuffer)(GL_READ_FRAMEBUFFER, 0);
        if scissor {
            (g.disable)(GL_SCISSOR_TEST);
        }
        let (sw, sh) = (p.src_width as i32, p.src_height as i32);
        let (dw, dh) = (p.dst_width as i32, p.dst_height as i32);
        if version >= 3 {
            let filter = if (sw, sh) == (dw, dh) {
                GL_NEAREST
            } else {
                GL_LINEAR
            };
            (g.blit)(0, 0, sw, sh, 0, 0, dw, dh, GL_COLOR_BUFFER_BIT, filter);
        } else {
            // ANGLE_framebuffer_blit copies without scaling.
            let (w, h) = (sw.min(dw), sh.min(dh));
            (g.blit_angle)(0, 0, w, h, 0, 0, w, h, GL_COLOR_BUFFER_BIT, GL_NEAREST);
        }
        if scissor {
            (g.enable)(GL_SCISSOR_TEST);
        }
        (g.bind_framebuffer)(GL_DRAW_FRAMEBUFFER, draw as u32);
        (g.bind_framebuffer)(GL_READ_FRAMEBUFFER, read as u32);
        match fence::fence(display) {
            Ok(f) => match aim_sync_file::give_to_guest(f){Ok(fd)=>p.fence=fd,Err(error)=>return -(error as i64)},
            Err(e) => return e,
        }
    }
    0
}
