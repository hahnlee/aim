//! bionic's `prop_area` (libc/system_properties/prop_area.cpp, prop_info.h,
//! system_properties.cpp `Add`/`Update`), writer and reader.
//!
//! Layout of one area file (`/dev/__properties__/<context>` and
//! `properties_serial`), all integers native-endian (little-endian on
//! arm64):
//!
//! ```text
//! 0    u32 bytes_used          data bytes allocated so far
//! 4    u32 serial              area serial (global serial in properties_serial)
//! 8    u32 magic   0x504f5250  "PROP"
//! 12   u32 version 0xfc6ed0ab
//! 16   u32 reserved[28]
//! 128  data[]                  offsets below are relative to data
//!      0   prop_trie_node root (namelen 0)
//!      20  dirty backup area   PROP_VALUE_MAX (92) bytes
//!      112 first allocation
//! ```
//!
//! `prop_trie_node` is `{u32 namelen; u32 prop, left, right, children; char
//! name[namelen + 1]}` padded to 4 bytes; each '.'-separated name segment is
//! a node, siblings form a binary search tree ordered by (length, strncmp).
//! `prop_info` is `{u32 serial; char value[92]; char name[]}` padded to 4
//! bytes; `serial` holds the value length in its top byte, a dirty bit in
//! bit 0 and `kLongFlag` (bit 16) for values of 92 bytes or more, which live
//! in a separate allocation whose offset (relative to the prop_info) is
//! stored at byte 60, after a 56-byte legacy error message.

use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering, fence};

/// `PA_SIZE` (bionic without `LARGE_SYSTEM_PROPERTY_NODE`).
pub const PA_SIZE: usize = 128 * 1024;
pub const PROP_AREA_MAGIC: u32 = 0x504f_5250;
pub const PROP_AREA_VERSION: u32 = 0xfc6e_d0ab;
/// `PROP_VALUE_MAX`.
pub const PROP_VALUE_MAX: usize = 92;
/// `PROP_NAME_MAX` (only limits the legacy protocol and `__system_property_read`).
pub const PROP_NAME_MAX: usize = 32;
/// `sizeof(prop_area)`.
pub const PROP_AREA_HEADER_SIZE: usize = 128;
const TRIE_NODE_SIZE: usize = 20;
const PROP_INFO_SIZE: usize = 96;
/// `prop_info::kLongFlag`.
pub const LONG_FLAG: u32 = 1 << 16;
/// `kLongLegacyError`.
pub const LONG_LEGACY_ERROR: &str = "Must use __system_property_read_callback() to read";
const LONG_OFFSET_FIELD: usize = 4 + 56;
/// Data offset of the dirty backup area (just after the root node).
const DIRTY_BACKUP_AREA: usize = TRIE_NODE_SIZE;

const fn align4(size: usize) -> usize {
    (size + 3) & !3
}

/// Memory backing one area: a heap buffer here, a shared `MAP_SHARED`
/// mapping of the `/dev/__properties__` file in the daemon.
///
/// # Safety
/// `base()` must point to `len()` bytes, 4-byte aligned, valid and writable
/// for the lifetime of the value, and not mutated by anyone but this writer
/// (readers in other processes may read concurrently, as with bionic).
#[allow(clippy::len_without_is_empty)]
pub unsafe trait AreaMemory {
    fn base(&self) -> *mut u8;
    fn len(&self) -> usize;
}

/// Zero-initialized heap memory for an area.
pub struct HeapMemory {
    words: Box<[AtomicU32]>,
}

impl HeapMemory {
    pub fn new(len: usize) -> Self {
        let words = (0..len.div_ceil(4)).map(|_| AtomicU32::new(0)).collect();
        Self { words }
    }
}

unsafe impl AreaMemory for HeapMemory {
    fn base(&self) -> *mut u8 {
        self.words.as_ptr() as *mut u8
    }
    fn len(&self) -> usize {
        self.words.len() * 4
    }
}

/// A writable `prop_area`, as init holds it.
pub struct PropArea<M: AreaMemory> {
    memory: M,
    data_size: usize,
}

impl<M: AreaMemory> PropArea<M> {
    /// `map_prop_area_rw` + the `prop_area` constructor on zeroed memory.
    pub fn create(memory: M) -> Self {
        assert!(memory.len() >= PROP_AREA_HEADER_SIZE + DIRTY_BACKUP_AREA + PROP_VALUE_MAX);
        assert_eq!(memory.base() as usize % 4, 0);
        // SAFETY: AreaMemory guarantees len() writable bytes.
        unsafe { ptr::write_bytes(memory.base(), 0, memory.len()) };
        let area = Self {
            data_size: memory.len() - PROP_AREA_HEADER_SIZE,
            memory,
        };
        area.header(8).store(PROP_AREA_MAGIC, Ordering::Relaxed);
        area.header(12).store(PROP_AREA_VERSION, Ordering::Relaxed);
        area.header(4).store(0, Ordering::Relaxed);
        area.header(0).store(
            (TRIE_NODE_SIZE + align4(PROP_VALUE_MAX)) as u32,
            Ordering::Relaxed,
        );
        area
    }

    pub fn memory(&self) -> &M {
        &self.memory
    }

    /// The whole area as bytes (a snapshot for writing the file).
    pub fn bytes(&self) -> Vec<u8> {
        // SAFETY: see AreaMemory.
        unsafe { std::slice::from_raw_parts(self.memory.base(), self.memory.len()).to_vec() }
    }

    fn header(&self, offset: usize) -> &AtomicU32 {
        // SAFETY: header fields are 4-byte aligned words inside the area.
        unsafe { AtomicU32::from_ptr(self.memory.base().add(offset) as *mut u32) }
    }

    /// A 4-byte aligned word at a data offset.
    fn word(&self, data_offset: usize) -> &AtomicU32 {
        debug_assert!(data_offset % 4 == 0 && data_offset + 4 <= self.data_size);
        // SAFETY: in bounds and aligned (all objects are 4-byte aligned).
        unsafe {
            AtomicU32::from_ptr(
                self.memory.base().add(PROP_AREA_HEADER_SIZE + data_offset) as *mut u32
            )
        }
    }

    fn data_ptr(&self, data_offset: usize) -> *mut u8 {
        // SAFETY: callers pass offsets within data.
        unsafe { self.memory.base().add(PROP_AREA_HEADER_SIZE + data_offset) }
    }

    fn write_bytes(&self, data_offset: usize, bytes: &[u8]) {
        assert!(data_offset + bytes.len() <= self.data_size);
        // SAFETY: bounds checked above.
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), self.data_ptr(data_offset), bytes.len())
        };
    }

    fn read_bytes(&self, data_offset: usize, len: usize) -> Vec<u8> {
        assert!(data_offset + len <= self.data_size);
        let mut out = vec![0u8; len];
        // SAFETY: bounds checked above.
        unsafe { ptr::copy_nonoverlapping(self.data_ptr(data_offset), out.as_mut_ptr(), len) };
        out
    }

    fn c_string_at(&self, data_offset: usize) -> Vec<u8> {
        let mut out = Vec::new();
        let mut offset = data_offset;
        while offset < self.data_size {
            // SAFETY: offset < data_size.
            let byte = unsafe { *self.data_ptr(offset) };
            if byte == 0 {
                break;
            }
            out.push(byte);
            offset += 1;
        }
        out
    }

    pub fn bytes_used(&self) -> u32 {
        self.header(0).load(Ordering::Relaxed)
    }

    /// The area serial word (`prop_area::serial()`), header offset 4.
    pub fn serial(&self) -> &AtomicU32 {
        self.header(4)
    }

    /// `allocate_obj`.
    fn allocate(&self, size: usize) -> Option<usize> {
        let aligned = align4(size);
        let used = self.bytes_used() as usize;
        if used + aligned > self.data_size {
            return None;
        }
        self.header(0)
            .store((used + aligned) as u32, Ordering::Relaxed);
        Some(used)
    }

    /// `new_prop_trie_node`.
    fn new_trie_node(&self, name: &[u8]) -> Option<usize> {
        let offset = self.allocate(TRIE_NODE_SIZE + name.len() + 1)?;
        self.word(offset)
            .store(name.len() as u32, Ordering::Relaxed);
        self.write_bytes(offset + TRIE_NODE_SIZE, name);
        self.write_bytes(offset + TRIE_NODE_SIZE + name.len(), &[0]);
        Some(offset)
    }

    /// `new_prop_info`.
    fn new_prop_info(&self, name: &[u8], value: &[u8]) -> Option<usize> {
        let offset = self.allocate(PROP_INFO_SIZE + name.len() + 1)?;
        if value.len() >= PROP_VALUE_MAX {
            let long_offset = self.allocate(value.len() + 1)?;
            self.write_bytes(long_offset, value);
            self.write_bytes(long_offset + value.len(), &[0]);
            let relative = (long_offset - offset) as u32;
            self.write_bytes(offset + PROP_INFO_SIZE, name);
            self.write_bytes(offset + PROP_INFO_SIZE + name.len(), &[0]);
            let error_len = LONG_LEGACY_ERROR.len() as u32;
            self.word(offset)
                .store((error_len << 24) | LONG_FLAG, Ordering::Relaxed);
            self.write_bytes(offset + 4, LONG_LEGACY_ERROR.as_bytes());
            self.write_bytes(offset + 4 + LONG_LEGACY_ERROR.len(), &[0]);
            self.word(offset + LONG_OFFSET_FIELD)
                .store(relative, Ordering::Relaxed);
        } else {
            self.write_bytes(offset + PROP_INFO_SIZE, name);
            self.write_bytes(offset + PROP_INFO_SIZE + name.len(), &[0]);
            self.word(offset)
                .store((value.len() as u32) << 24, Ordering::Relaxed);
            self.write_bytes(offset + 4, value);
            self.write_bytes(offset + 4 + value.len(), &[0]);
        }
        Some(offset)
    }

    fn node_name(&self, node: usize) -> Vec<u8> {
        let len = self.word(node).load(Ordering::Relaxed) as usize;
        self.read_bytes(node + TRIE_NODE_SIZE, len)
    }

    /// `to_prop_obj` bounds rule: offsets past `pa_data_size_` are null.
    fn valid(&self, offset: u32) -> Option<usize> {
        ((offset as usize) <= self.data_size).then_some(offset as usize)
    }

    /// `find_prop_trie_node` among the siblings rooted at `trie`.
    fn find_trie_node(&self, trie: usize, name: &[u8], alloc: bool) -> Option<usize> {
        let mut current = trie;
        loop {
            let ordering = cmp_prop_name(name, &self.node_name(current));
            if ordering == 0 {
                return Some(current);
            }
            let link = if ordering < 0 {
                current + 8
            } else {
                current + 12
            };
            let offset = self.word(link).load(Ordering::Acquire);
            if offset != 0 {
                current = self.valid(offset)?;
            } else {
                if !alloc {
                    return None;
                }
                let new_node = self.new_trie_node(name)?;
                self.word(link).store(new_node as u32, Ordering::Release);
                return Some(new_node);
            }
        }
    }

    /// `find_property`; returns the prop_info data offset.
    fn find_property(&self, name: &[u8], value: &[u8], alloc: bool) -> Option<usize> {
        let mut remaining = name;
        let mut current = 0usize;
        loop {
            let separator = remaining.iter().position(|b| *b == b'.');
            let segment = &remaining[..separator.unwrap_or(remaining.len())];
            if segment.is_empty() {
                return None;
            }
            let children = self.word(current + 16).load(Ordering::Acquire);
            let root = if children != 0 {
                self.valid(children)?
            } else if alloc {
                let node = self.new_trie_node(segment)?;
                self.word(current + 16)
                    .store(node as u32, Ordering::Release);
                node
            } else {
                return None;
            };
            current = self.find_trie_node(root, segment, alloc)?;
            match separator {
                Some(sep) => remaining = &remaining[sep + 1..],
                None => break,
            }
        }
        let prop = self.word(current + 4).load(Ordering::Acquire);
        if prop != 0 {
            return self.valid(prop);
        }
        if !alloc {
            return None;
        }
        let info = self.new_prop_info(name, value)?;
        self.word(current + 4).store(info as u32, Ordering::Release);
        Some(info)
    }

    /// `prop_area::find`: the prop_info data offset.
    pub fn find(&self, name: &str) -> Option<usize> {
        self.find_property(name.as_bytes(), &[], false)
    }

    /// `prop_area::add`: false when out of space (partial allocations stay,
    /// as in bionic).
    pub fn add(&self, name: &str, value: &[u8]) -> bool {
        self.find_property(name.as_bytes(), value, true).is_some()
    }

    /// The serial word of the prop_info at `info`.
    pub fn info_serial(&self, info: usize) -> &AtomicU32 {
        self.word(info)
    }

    /// Byte offset of a prop_info's serial within the area file (for futex
    /// wake addressing).
    pub fn file_offset(&self, info: usize) -> usize {
        PROP_AREA_HEADER_SIZE + info
    }

    pub fn info_name(&self, info: usize) -> String {
        String::from_utf8_lossy(&self.c_string_at(info + PROP_INFO_SIZE)).into_owned()
    }

    /// The current value of a prop_info (long values included).
    pub fn info_value(&self, info: usize) -> Vec<u8> {
        let serial = self.info_serial(info).load(Ordering::Acquire);
        if serial & LONG_FLAG != 0 && self.info_name(info).starts_with("ro.") {
            let relative = self.word(info + LONG_OFFSET_FIELD).load(Ordering::Relaxed) as usize;
            return self.c_string_at(info + relative);
        }
        let len = (serial >> 24) as usize;
        self.read_bytes(info + 4, len)
    }

    /// The first half of `SystemProperties::Update` for this prop_info:
    /// back up the old value, mark dirty, write, publish the new serial.
    /// Returns the new serial. The caller bumps the global serial and wakes.
    pub(crate) fn update_value(&self, info: usize, value: &[u8]) -> u32 {
        let serial_word = self.info_serial(info);
        let serial = serial_word.load(Ordering::Relaxed);
        let old_len = (serial >> 24) as usize;
        // memcpy(pa->dirty_backup_area(), pi->value, old_len + 1)
        let old = self.read_bytes(info + 4, old_len + 1);
        self.write_bytes(DIRTY_BACKUP_AREA, &old);
        fence(Ordering::Release);
        let serial = serial | 1;
        serial_word.store(serial, Ordering::Relaxed);
        // strlcpy(pi->value, value, len + 1)
        let copied = value.iter().position(|b| *b == 0).unwrap_or(value.len());
        self.write_bytes(info + 4, &value[..copied]);
        self.write_bytes(info + 4 + copied, &[0]);
        fence(Ordering::Release);
        let new_serial = ((value.len() as u32) << 24) | (serial.wrapping_add(1) & 0x00ff_ffff);
        serial_word.store(new_serial, Ordering::Relaxed);
        new_serial
    }

    /// `prop_area::foreach` order: left subtree, node's property, children,
    /// right subtree. Returns prop_info data offsets.
    pub fn foreach(&self) -> Vec<usize> {
        let mut out = Vec::new();
        self.foreach_node(0, &mut out);
        out
    }

    fn foreach_node(&self, node: usize, out: &mut Vec<usize>) {
        let left = self.word(node + 8).load(Ordering::Acquire);
        if left != 0
            && let Some(left) = self.valid(left)
        {
            self.foreach_node(left, out);
        }
        let prop = self.word(node + 4).load(Ordering::Acquire);
        if prop != 0
            && let Some(prop) = self.valid(prop)
        {
            out.push(prop);
        }
        let children = self.word(node + 16).load(Ordering::Acquire);
        if children != 0
            && let Some(children) = self.valid(children)
        {
            self.foreach_node(children, out);
        }
        let right = self.word(node + 12).load(Ordering::Acquire);
        if right != 0
            && let Some(right) = self.valid(right)
        {
            self.foreach_node(right, out);
        }
    }
}

/// `cmp_prop_name`: shorter names sort first, then `strncmp`.
fn cmp_prop_name(one: &[u8], two: &[u8]) -> i32 {
    if one.len() < two.len() {
        return -1;
    }
    if one.len() > two.len() {
        return 1;
    }
    for (a, b) in one.iter().zip(two) {
        if a != b {
            return *a as i32 - *b as i32;
        }
        if *a == 0 {
            return 0;
        }
    }
    0
}

// ---------------------------------------------------------------------------
// Reader

/// One property as a reader sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadProperty {
    pub name: String,
    pub value: String,
    pub serial: u32,
}

/// A read-only area, following bionic's reader (`map_fd_ro`, `find`,
/// `foreach`, `ReadMutablePropertyValue`, `ReadCallback`) over raw bytes.
pub struct PropAreaReader<'a> {
    bytes: &'a [u8],
}

impl<'a> PropAreaReader<'a> {
    /// `map_fd_ro` checks: at least the header, magic and version.
    pub fn new(bytes: &'a [u8]) -> Result<Self, String> {
        if bytes.len() < PROP_AREA_HEADER_SIZE {
            return Err("prop_area smaller than its header".to_string());
        }
        let reader = Self { bytes };
        if reader.header(8) != PROP_AREA_MAGIC || reader.header(12) != PROP_AREA_VERSION {
            return Err("bad prop_area magic or version".to_string());
        }
        Ok(reader)
    }

    fn header(&self, offset: usize) -> u32 {
        u32::from_ne_bytes(self.bytes[offset..offset + 4].try_into().unwrap())
    }

    fn data(&self) -> &'a [u8] {
        &self.bytes[PROP_AREA_HEADER_SIZE..]
    }

    fn u32_at(&self, offset: usize) -> Option<u32> {
        let data = self.data();
        data.get(offset..offset + 4)
            .map(|b| u32::from_ne_bytes(b.try_into().unwrap()))
    }

    /// `to_prop_obj`: offsets beyond `pa_data_size_` are null.
    fn obj(&self, offset: u32) -> Option<usize> {
        let offset = offset as usize;
        (offset <= self.data().len()).then_some(offset)
    }

    fn c_str(&self, offset: usize) -> &'a [u8] {
        let data = self.data();
        let rest = &data[offset.min(data.len())..];
        &rest[..rest.iter().position(|b| *b == 0).unwrap_or(rest.len())]
    }

    pub fn bytes_used(&self) -> u32 {
        self.header(0)
    }

    pub fn serial(&self) -> u32 {
        self.header(4)
    }

    fn node_name(&self, node: usize) -> Option<&'a [u8]> {
        let len = self.u32_at(node)? as usize;
        self.data()
            .get(node + TRIE_NODE_SIZE..node + TRIE_NODE_SIZE + len)
    }

    /// `prop_area::find`: the prop_info offset for `name`.
    pub fn find(&self, name: &str) -> Option<usize> {
        let mut remaining = name.as_bytes();
        let mut current = 0usize;
        loop {
            let separator = remaining.iter().position(|b| *b == b'.');
            let segment = &remaining[..separator.unwrap_or(remaining.len())];
            if segment.is_empty() {
                return None;
            }
            let children = self.u32_at(current + 16)?;
            if children == 0 {
                return None;
            }
            let mut node = self.obj(children)?;
            loop {
                let ordering = cmp_prop_name(segment, self.node_name(node)?);
                if ordering == 0 {
                    break;
                }
                let link = self.u32_at(if ordering < 0 { node + 8 } else { node + 12 })?;
                if link == 0 {
                    return None;
                }
                node = self.obj(link)?;
            }
            current = node;
            match separator {
                Some(sep) => remaining = &remaining[sep + 1..],
                None => break,
            }
        }
        let prop = self.u32_at(current + 4)?;
        if prop == 0 {
            return None;
        }
        self.obj(prop)
    }

    /// `SystemProperties::ReadCallback` for one prop_info: read-only names
    /// use the long value when flagged; mutable ones go through the dirty
    /// backup area while the serial is dirty.
    pub fn read(&self, info: usize) -> Option<ReadProperty> {
        let name = String::from_utf8_lossy(self.c_str(info + PROP_INFO_SIZE)).into_owned();
        let serial = self.u32_at(info)?;
        let value_bytes: &[u8] = if name.starts_with("ro.") {
            if serial & LONG_FLAG != 0 {
                let relative = self.u32_at(info + LONG_OFFSET_FIELD)? as usize;
                self.c_str(info + relative)
            } else {
                self.c_str(info + 4)
            }
        } else {
            let len = (serial >> 24) as usize;
            let source = if serial & 1 != 0 {
                DIRTY_BACKUP_AREA
            } else {
                info + 4
            };
            // ReadMutablePropertyValue copies len + 1 bytes into a
            // PROP_VALUE_MAX buffer; the callback sees it as a C string.
            let raw = self.data().get(source..source + len + 1)?;
            &raw[..raw.iter().position(|b| *b == 0).unwrap_or(raw.len())]
        };
        Some(ReadProperty {
            name,
            value: String::from_utf8_lossy(value_bytes).into_owned(),
            serial,
        })
    }

    /// `__system_property_get`-style lookup.
    pub fn get(&self, name: &str) -> Option<ReadProperty> {
        self.find(name).and_then(|info| self.read(info))
    }

    /// `prop_area::foreach` order.
    pub fn foreach(&self) -> Vec<ReadProperty> {
        let mut out = Vec::new();
        self.walk(0, &mut out, 0);
        out
    }

    fn walk(&self, node: usize, out: &mut Vec<ReadProperty>, depth: usize) {
        if depth > 4096 {
            return;
        }
        let field = |o: usize| self.u32_at(node + o).unwrap_or(0);
        if field(8) != 0
            && let Some(left) = self.obj(field(8))
        {
            self.walk(left, out, depth + 1);
        }
        if field(4) != 0
            && let Some(info) = self.obj(field(4)).and_then(|info| self.read(info))
        {
            out.push(info);
        }
        if field(16) != 0
            && let Some(children) = self.obj(field(16))
        {
            self.walk(children, out, depth + 1);
        }
        if field(12) != 0
            && let Some(right) = self.obj(field(12))
        {
            self.walk(right, out, depth + 1);
        }
    }
}
