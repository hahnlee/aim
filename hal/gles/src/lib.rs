//! `libGLES_aim.so`: the GLES driver of the derived image
//! (`docs/gles-driver.md`). The original `/system/lib64/libEGL.so` loads it
//! from `/vendor/lib64/egl` like any device's GPU driver.
//!
//! Every EGL and GLES entry point is a thunk that makes one host call to the
//! host module `gpu`, which calls the host's ANGLE (OpenGL ES on Metal).
//! Guest pointers are host pointers, so arguments pass through untouched.
//! The thunks are generated from the Khronos registry
//! (`tools/gen-gpu-thunks.py`); [`egl`], [`gl`] and [`external`] hold what
//! the guest does itself: the Android platform (native windows and buffers),
//! callbacks, and external textures.

pub mod buffers;
pub mod egl;
pub mod external;
pub mod gl;
mod hostcall;
#[rustfmt::skip]
pub mod thunks;
pub mod types;
