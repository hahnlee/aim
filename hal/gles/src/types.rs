//! Khronos EGL and GLES types (`EGL/egl.h`, `GLES3/gl3.h`) as they are on
//! arm64 Android.
#![allow(clippy::upper_case_acronyms)]

use core::ffi::c_void;

pub type GLenum = u32;
pub type GLboolean = u8;
pub type GLbitfield = u32;
pub type GLbyte = i8;
pub type GLubyte = u8;
pub type GLshort = i16;
pub type GLushort = u16;
pub type GLint = i32;
pub type GLuint = u32;
pub type GLfixed = i32;
pub type GLclampx = i32;
pub type GLsizei = i32;
pub type GLfloat = f32;
pub type GLclampf = f32;
pub type GLintptr = isize;
pub type GLsizeiptr = isize;
pub type GLint64 = i64;
pub type GLuint64 = u64;
pub type GLchar = i8;
pub type GLhalf = u16;
pub type GLsync = *mut c_void;
pub type GLeglImageOES = *mut c_void;
pub type GLeglClientBufferEXT = *mut c_void;

pub type EGLBoolean = u32;
pub type EGLint = i32;
pub type EGLenum = u32;
pub type EGLAttrib = isize;
pub type EGLAttribKHR = isize;
pub type EGLTime = u64;
pub type EGLTimeKHR = u64;
pub type EGLuint64KHR = u64;
pub type EGLnsecsANDROID = i64;
pub type EGLDisplay = *mut c_void;
pub type EGLConfig = *mut c_void;
pub type EGLSurface = *mut c_void;
pub type EGLContext = *mut c_void;
pub type EGLClientBuffer = *mut c_void;
pub type EGLImage = *mut c_void;
pub type EGLImageKHR = *mut c_void;
pub type EGLSync = *mut c_void;
pub type EGLSyncKHR = *mut c_void;
pub type EGLStreamKHR = *mut c_void;
pub type EGLDeviceEXT = *mut c_void;
pub type EGLLabelKHR = *mut c_void;
pub type EGLObjectKHR = *mut c_void;
pub type EGLNativeDisplayType = *mut c_void;
pub type EGLNativeWindowType = *mut c_void;
pub type EGLNativePixmapType = *mut c_void;

/// An exported entry point, for `eglGetProcAddress`.
pub struct Proc {
    pub name: &'static str,
    pub addr: *const c_void,
    /// Host table id, whose resolution decides availability; -1 for an
    /// entry point implemented in the guest.
    pub id: i32,
}

// SAFETY: immutable addresses of functions.
unsafe impl Sync for Proc {}
