//! What `vold` (src/main.rs) does without binder, built for the host too so
//! its tests run there: user storage directories and file-based encryption
//! keys.

pub mod fscrypt;
pub mod key_storage;
pub mod storage;

#[cfg(test)]
mod tests;
