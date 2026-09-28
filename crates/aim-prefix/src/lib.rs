use std::ffi::{CStr, c_char};
use std::ptr;

// Linux PATH_MAX includes the terminating NUL byte.
const MAX_GUEST_PATH_BYTES: usize = 4095;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum MountKind {
    Immutable = 1,
    Private = 2,
    Shared = 3,
    Synthetic = 4,
}

impl MountKind {
    fn from_raw(value: u32) -> Option<Self> {
        match value {
            1 => Some(Self::Immutable),
            2 => Some(Self::Private),
            3 => Some(Self::Shared),
            4 => Some(Self::Synthetic),
            _ => None,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PrefixError {
    InvalidPath,
    DuplicateMount,
    TableSealed,
    TableNotSealed,
    NoMount,
}

#[derive(Clone, Debug)]
struct Mount {
    id: u32,
    kind: MountKind,
    writable: bool,
    components: Vec<Vec<u8>>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct Resolution {
    pub mount_id: u32,
    pub kind: MountKind,
    pub writable: bool,
    pub requires_directory: bool,
    pub normalized_path: Vec<u8>,
    pub relative_path: Vec<u8>,
}

#[derive(Default)]
pub struct MountTable {
    mounts: Vec<Mount>,
    sealed: bool,
}

impl MountTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_mount(
        &mut self,
        id: u32,
        kind: MountKind,
        writable: bool,
        guest_prefix: &[u8],
    ) -> Result<(), PrefixError> {
        if self.sealed {
            return Err(PrefixError::TableSealed);
        }
        let components = normalize_absolute(guest_prefix)?;
        if self
            .mounts
            .iter()
            .any(|mount| mount.id == id || mount.components == components)
        {
            return Err(PrefixError::DuplicateMount);
        }
        self.mounts.push(Mount {
            id,
            kind,
            writable,
            components,
        });
        Ok(())
    }

    pub fn seal(&mut self) -> Result<(), PrefixError> {
        if self.sealed {
            return Err(PrefixError::TableSealed);
        }
        self.mounts
            .sort_by(|left, right| right.components.len().cmp(&left.components.len()));
        self.sealed = true;
        Ok(())
    }

    pub fn resolve(&self, cwd: &[u8], path: &[u8]) -> Result<Resolution, PrefixError> {
        if !self.sealed {
            return Err(PrefixError::TableNotSealed);
        }
        let components = normalize(cwd, path)?;
        let mount = self
            .mounts
            .iter()
            .find(|mount| components.starts_with(&mount.components))
            .ok_or(PrefixError::NoMount)?;
        let normalized_path = join_absolute(&components);
        let relative_path = join_relative(&components[mount.components.len()..]);
        Ok(Resolution {
            mount_id: mount.id,
            kind: mount.kind,
            writable: mount.writable,
            requires_directory: path.len() > 1 && path.ends_with(b"/"),
            normalized_path,
            relative_path,
        })
    }
}

fn normalize(cwd: &[u8], path: &[u8]) -> Result<Vec<Vec<u8>>, PrefixError> {
    validate_text(path)?;
    let mut components = if path.starts_with(b"/") {
        Vec::new()
    } else {
        normalize_absolute(cwd)?
    };
    apply_components(&mut components, path)?;
    validate_normalized_length(&components)?;
    Ok(components)
}

fn normalize_absolute(path: &[u8]) -> Result<Vec<Vec<u8>>, PrefixError> {
    validate_text(path)?;
    if !path.starts_with(b"/") {
        return Err(PrefixError::InvalidPath);
    }
    let mut components = Vec::new();
    apply_components(&mut components, path)?;
    validate_normalized_length(&components)?;
    Ok(components)
}

fn validate_text(path: &[u8]) -> Result<(), PrefixError> {
    if path.is_empty() || path.len() > MAX_GUEST_PATH_BYTES || path.contains(&0) {
        return Err(PrefixError::InvalidPath);
    }
    Ok(())
}

fn apply_components(components: &mut Vec<Vec<u8>>, path: &[u8]) -> Result<(), PrefixError> {
    for component in path.split(|byte| *byte == b'/') {
        match component {
            b"" | b"." => {}
            b".." => {
                // Linux namei keeps an absolute lookup at its process root
                // when another `..` is encountered. Our virtual root is the
                // equivalent containment boundary, so this both preserves
                // guest semantics and prevents escape into a host path.
                components.pop();
            }
            value => components.push(value.to_vec()),
        }
    }
    Ok(())
}

fn validate_normalized_length(components: &[Vec<u8>]) -> Result<(), PrefixError> {
    let bytes = 1
        + components.iter().map(|part| part.len()).sum::<usize>()
        + components.len().saturating_sub(1);
    if bytes > MAX_GUEST_PATH_BYTES {
        Err(PrefixError::InvalidPath)
    } else {
        Ok(())
    }
}

fn join_absolute(components: &[Vec<u8>]) -> Vec<u8> {
    if components.is_empty() {
        b"/".to_vec()
    } else {
        let mut result = Vec::with_capacity(
            1 + components.iter().map(Vec::len).sum::<usize>() + components.len() - 1,
        );
        result.push(b'/');
        result.extend(join_relative(components));
        result
    }
}

fn join_relative(components: &[Vec<u8>]) -> Vec<u8> {
    let mut result = Vec::new();
    for (index, component) in components.iter().enumerate() {
        if index != 0 {
            result.push(b'/');
        }
        result.extend(component);
    }
    result
}

#[repr(C)]
pub struct AimPrefixResolution {
    mount_id: u32,
    mount_kind: u32,
    writable: bool,
    requires_directory: bool,
    normalized_path_length: usize,
    relative_path_length: usize,
}

#[derive(Clone, Copy)]
#[repr(i32)]
pub enum AimPrefixResult {
    Ok = 0,
    InvalidArgument = 1,
    InvalidPath = 2,
    DuplicateMount = 3,
    TableSealed = 4,
    TableNotSealed = 5,
    NoMount = 6,
    BufferTooSmall = 7,
}

impl From<PrefixError> for AimPrefixResult {
    fn from(error: PrefixError) -> Self {
        match error {
            PrefixError::InvalidPath => Self::InvalidPath,
            PrefixError::DuplicateMount => Self::DuplicateMount,
            PrefixError::TableSealed => Self::TableSealed,
            PrefixError::TableNotSealed => Self::TableNotSealed,
            PrefixError::NoMount => Self::NoMount,
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn aim_prefix_create() -> *mut MountTable {
    Box::into_raw(Box::new(MountTable::new()))
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `prefix` must be null or a live pointer returned by
/// [`aim_prefix_create`] that has not already been destroyed.
pub unsafe extern "C" fn aim_prefix_destroy(prefix: *mut MountTable) {
    if !prefix.is_null() {
        // SAFETY: The C ABI requires a pointer returned by create exactly once.
        drop(unsafe { Box::from_raw(prefix) });
    }
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `prefix` must point to a live table and `guest_prefix` must point to a
/// readable NUL-terminated UTF-8 string for the duration of this call.
pub unsafe extern "C" fn aim_prefix_add_mount(
    prefix: *mut MountTable,
    mount_id: u32,
    kind: u32,
    writable: bool,
    guest_prefix: *const c_char,
) -> AimPrefixResult {
    let Some(prefix) = (unsafe { prefix.as_mut() }) else {
        return AimPrefixResult::InvalidArgument;
    };
    let Some(kind) = MountKind::from_raw(kind) else {
        return AimPrefixResult::InvalidArgument;
    };
    let Ok(guest_prefix) = c_bytes(guest_prefix) else {
        return AimPrefixResult::InvalidArgument;
    };
    prefix
        .add_mount(mount_id, kind, writable, &guest_prefix)
        .map_or_else(Into::into, |_| AimPrefixResult::Ok)
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `prefix` must point to a live table exclusively owned by the caller for the
/// duration of this mutation.
pub unsafe extern "C" fn aim_prefix_seal(prefix: *mut MountTable) -> AimPrefixResult {
    let Some(prefix) = (unsafe { prefix.as_mut() }) else {
        return AimPrefixResult::InvalidArgument;
    };
    prefix
        .seal()
        .map_or_else(Into::into, |_| AimPrefixResult::Ok)
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `prefix` must point to a live sealed table; `cwd` and `path` must be
/// readable NUL-terminated UTF-8 strings; output pointers must be writable for
/// the capacities supplied and must not overlap.
pub unsafe extern "C" fn aim_prefix_resolve(
    prefix: *const MountTable,
    cwd: *const c_char,
    path: *const c_char,
    resolution: *mut AimPrefixResolution,
    normalized_path: *mut c_char,
    normalized_path_capacity: usize,
    relative_path: *mut c_char,
    relative_path_capacity: usize,
) -> AimPrefixResult {
    let Some(prefix) = (unsafe { prefix.as_ref() }) else {
        return AimPrefixResult::InvalidArgument;
    };
    let (Ok(cwd), Ok(path)) = (c_bytes(cwd), c_bytes(path)) else {
        return AimPrefixResult::InvalidArgument;
    };
    let resolved = match prefix.resolve(&cwd, &path) {
        Ok(resolved) => resolved,
        Err(error) => return error.into(),
    };
    let normalized = resolved.normalized_path.as_slice();
    let relative = resolved.relative_path.as_slice();
    if resolution.is_null()
        || normalized_path_capacity <= normalized.len()
        || relative_path_capacity <= relative.len()
        || (normalized_path.is_null() && normalized_path_capacity != 0)
        || (relative_path.is_null() && relative_path_capacity != 0)
    {
        return AimPrefixResult::BufferTooSmall;
    }
    // SAFETY: Capacities were checked above and the C ABI requires writable
    // buffers of those sizes. copy_to_c_buffer always appends one NUL byte.
    unsafe {
        copy_to_c_buffer(normalized, normalized_path.cast());
        copy_to_c_buffer(relative, relative_path.cast());
        ptr::write(
            resolution,
            AimPrefixResolution {
                mount_id: resolved.mount_id,
                mount_kind: resolved.kind as u32,
                writable: resolved.writable,
                requires_directory: resolved.requires_directory,
                normalized_path_length: normalized.len(),
                relative_path_length: relative.len(),
            },
        );
    }
    AimPrefixResult::Ok
}

fn c_bytes(value: *const c_char) -> Result<Vec<u8>, ()> {
    if value.is_null() {
        return Err(());
    }
    // SAFETY: The C ABI requires a readable NUL-terminated string.
    Ok(unsafe { CStr::from_ptr(value) }.to_bytes().to_vec())
}

unsafe fn copy_to_c_buffer(source: &[u8], destination: *mut u8) {
    // SAFETY: The caller checked destination capacity for source plus NUL.
    unsafe {
        ptr::copy_nonoverlapping(source.as_ptr(), destination, source.len());
        destination.add(source.len()).write(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> MountTable {
        let mut table = MountTable::new();
        table
            .add_mount(1, MountKind::Immutable, false, b"/")
            .unwrap();
        table
            .add_mount(2, MountKind::Private, true, b"/data")
            .unwrap();
        table
            .add_mount(3, MountKind::Private, true, b"/data/user/0/app")
            .unwrap();
        table
            .add_mount(4, MountKind::Shared, true, b"/storage/host")
            .unwrap();
        table.seal().unwrap();
        table
    }

    #[test]
    fn longest_component_prefix_wins() {
        let resolved = table().resolve(b"/", b"/data/user/0/app/files/a").unwrap();
        assert_eq!(resolved.mount_id, 3);
        assert_eq!(resolved.relative_path, b"files/a");
    }

    #[test]
    fn normalizes_before_mount_selection() {
        let resolved = table()
            .resolve(b"/data/user/0/app", b"../other/./cache")
            .unwrap();
        assert_eq!(resolved.mount_id, 2);
        assert_eq!(resolved.normalized_path, b"/data/user/0/other/cache");
        assert_eq!(resolved.relative_path, b"user/0/other/cache");
    }

    #[test]
    fn mount_names_match_whole_components() {
        let resolved = table().resolve(b"/", b"/database/value").unwrap();
        assert_eq!(resolved.mount_id, 1);
        assert_eq!(resolved.relative_path, b"database/value");
    }

    #[test]
    fn dot_dot_clamps_at_virtual_root() {
        let resolved = table().resolve(b"/", b"../../host").unwrap();
        assert_eq!(resolved.mount_id, 1);
        assert_eq!(resolved.normalized_path, b"/host");
        assert_eq!(resolved.relative_path, b"host");
    }

    #[test]
    fn rejects_normalized_path_over_path_max() {
        let first = "a".repeat(3000);
        let second = "b".repeat(1096);
        assert_eq!(
            table().resolve(b"/", format!("/{first}/{second}").as_bytes()),
            Err(PrefixError::InvalidPath)
        );
    }

    #[test]
    fn preserves_non_utf8_names_and_directory_intent() {
        let resolved = table().resolve(b"/data", b"name-\xff/").unwrap();
        assert_eq!(resolved.normalized_path, b"/data/name-\xff");
        assert_eq!(resolved.relative_path, b"name-\xff");
        assert!(resolved.requires_directory);
    }

    #[test]
    fn table_must_be_sealed_and_unique() {
        let mut table = MountTable::new();
        table
            .add_mount(1, MountKind::Immutable, false, b"/system")
            .unwrap();
        assert_eq!(
            table.resolve(b"/", b"/system/lib64"),
            Err(PrefixError::TableNotSealed)
        );
        assert_eq!(
            table.add_mount(1, MountKind::Private, true, b"/data"),
            Err(PrefixError::DuplicateMount)
        );
    }
}
