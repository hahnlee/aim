//! The set of property areas init creates in `/dev/__properties__`
//! (bionic `ContextsSerialized` opened writable + `SystemProperties::Add` /
//! `Update`), with the futex wakes bionic performs.

use std::fs;
use std::io;
use std::path::Path;
use std::sync::atomic::Ordering;

use super::area::{AreaMemory, HeapMemory, PA_SIZE, PROP_VALUE_MAX, PropArea};
use super::info::PropertyInfoArea;

/// `PROP_DIRNAME`.
pub const PROP_DIRNAME: &str = "/dev/__properties__";
/// `properties_serial`, labeled `u:object_r:properties_serial:s0`.
pub const PROPERTIES_SERIAL: &str = "properties_serial";
pub const PROPERTIES_SERIAL_CONTEXT: &str = "u:object_r:properties_serial:s0";
/// `PROP_TREE_FILE` basename.
pub const PROPERTY_INFO: &str = "property_info";

/// A futex word some process may be sleeping on in
/// `__system_property_wait` / `__system_property_wait_any`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WakeTarget<'a> {
    /// The area file under `/dev/__properties__` (a context name or
    /// `properties_serial`).
    pub area: &'a str,
    /// Byte offset of the 32-bit serial word within that file.
    pub offset: usize,
}

/// The wake half of bionic's `__futex_wake(addr, INT32_MAX)`.
///
/// Semantics the syscall layer must provide: every thread in any process
/// blocked in `FUTEX_WAIT` (bionic uses the shared, non-private futex ops on
/// these `MAP_SHARED` file mappings) on the word at `target` is woken. The
/// store to the word has already happened with release ordering; waiters
/// re-read it and go back to sleep if it still equals their expected value.
/// Readers never write these words, so a wake with no sleepers is a no-op.
pub trait FutexWaker: Send {
    fn wake_all(&self, target: WakeTarget<'_>);
}

/// Discards wakes (nothing can be waiting on heap-only areas).
#[derive(Clone, Copy, Debug, Default)]
pub struct NoWake;

impl FutexWaker for NoWake {
    fn wake_all(&self, _target: WakeTarget<'_>) {}
}

/// Why `__system_property_add` / `__system_property_update` returned -1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AreaError {
    EmptyName,
    ValueTooLong,
    NoContext(String),
    AreaFull(String),
    NotFound(String),
}

/// All property areas plus the serialized `property_info`.
pub struct PropertyAreas<M: AreaMemory = HeapMemory> {
    info: Vec<u8>,
    contexts: Vec<String>,
    areas: Vec<PropArea<M>>,
    serial: PropArea<M>,
}

impl PropertyAreas<HeapMemory> {
    /// Areas of bionic's default size on the heap.
    pub fn new(property_info: Vec<u8>) -> Result<Self, String> {
        Self::with_memory(property_info, |_| HeapMemory::new(PA_SIZE))
    }
}

impl<M: AreaMemory> PropertyAreas<M> {
    /// `ContextsSerialized::Initialize(writable=true)`: one area per context
    /// in `property_info` index order, then `properties_serial`.
    /// `memory_for` receives the file name (context or `properties_serial`).
    pub fn with_memory(
        property_info: Vec<u8>,
        mut memory_for: impl FnMut(&str) -> M,
    ) -> Result<Self, String> {
        let contexts: Vec<String> = PropertyInfoArea::new(&property_info)?
            .contexts()
            .into_iter()
            .map(str::to_string)
            .collect();
        let areas = contexts
            .iter()
            .map(|context| PropArea::create(memory_for(context)))
            .collect();
        let serial = PropArea::create(memory_for(PROPERTIES_SERIAL));
        Ok(Self {
            info: property_info,
            contexts,
            areas,
            serial,
        })
    }

    pub fn property_info(&self) -> &[u8] {
        &self.info
    }

    pub fn info_area(&self) -> PropertyInfoArea<'_> {
        PropertyInfoArea::new(&self.info).expect("validated at construction")
    }

    pub fn contexts(&self) -> &[String] {
        &self.contexts
    }

    pub fn area(&self, context: &str) -> Option<&PropArea<M>> {
        let index = self.contexts.iter().position(|c| c == context)?;
        Some(&self.areas[index])
    }

    pub fn serial_area(&self) -> &PropArea<M> {
        &self.serial
    }

    /// `GetPropAreaForName`.
    fn area_index_for(&self, name: &str) -> Option<usize> {
        let (index, _) = self.info_area().property_info_indexes(name);
        ((index as usize) < self.areas.len()).then_some(index as usize)
    }

    /// `__system_property_find` + value read.
    pub fn get(&self, name: &str) -> Option<String> {
        let area = &self.areas[self.area_index_for(name)?];
        let info = area.find(name)?;
        Some(String::from_utf8_lossy(&area.info_value(info)).into_owned())
    }

    pub fn contains(&self, name: &str) -> bool {
        self.area_index_for(name)
            .and_then(|index| self.areas[index].find(name))
            .is_some()
    }

    /// The prop_info serial of a property.
    pub fn property_serial(&self, name: &str) -> Option<u32> {
        let area = &self.areas[self.area_index_for(name)?];
        let info = area.find(name)?;
        Some(area.info_serial(info).load(Ordering::Acquire))
    }

    /// The global serial (`__system_property_area_serial`).
    pub fn area_serial(&self) -> u32 {
        self.serial.serial().load(Ordering::Acquire)
    }

    fn bump_global_serial(&self, waker: &dyn FutexWaker) {
        let serial = self.serial.serial();
        serial.store(
            serial.load(Ordering::Relaxed).wrapping_add(1),
            Ordering::Release,
        );
        waker.wake_all(WakeTarget {
            area: PROPERTIES_SERIAL,
            offset: 4,
        });
    }

    /// `SystemProperties::Add` (no appcompat override areas: init writes
    /// those only when built with `WRITE_APPCOMPAT_OVERRIDE_SYSTEM_PROPERTIES`).
    pub fn add(&self, name: &str, value: &[u8], waker: &dyn FutexWaker) -> Result<(), AreaError> {
        if name.is_empty() {
            return Err(AreaError::EmptyName);
        }
        if value.len() >= PROP_VALUE_MAX && !name.starts_with("ro.") {
            return Err(AreaError::ValueTooLong);
        }
        let index = self
            .area_index_for(name)
            .ok_or_else(|| AreaError::NoContext(name.to_string()))?;
        if !self.areas[index].add(name, value) {
            return Err(AreaError::AreaFull(self.contexts[index].clone()));
        }
        self.bump_global_serial(waker);
        Ok(())
    }

    /// `SystemProperties::Update`: dirty-backup protocol on the prop_info,
    /// then wake its serial, bump the global serial and wake that.
    pub fn update(
        &self,
        name: &str,
        value: &[u8],
        waker: &dyn FutexWaker,
    ) -> Result<(), AreaError> {
        if value.len() >= PROP_VALUE_MAX {
            return Err(AreaError::ValueTooLong);
        }
        let index = self
            .area_index_for(name)
            .ok_or_else(|| AreaError::NoContext(name.to_string()))?;
        let area = &self.areas[index];
        let info = area
            .find(name)
            .ok_or_else(|| AreaError::NotFound(name.to_string()))?;
        area.update_value(info, value);
        waker.wake_all(WakeTarget {
            area: &self.contexts[index],
            offset: area.file_offset(info),
        });
        self.bump_global_serial(waker);
        Ok(())
    }

    /// Every property, context by context in index order, each area in
    /// `prop_area::foreach` order (what `__system_property_foreach` visits).
    pub fn foreach(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for area in &self.areas {
            for info in area.foreach() {
                out.push((
                    area.info_name(info),
                    String::from_utf8_lossy(&area.info_value(info)).into_owned(),
                ));
            }
        }
        out
    }

    /// Writes `property_info`, one file per context and `properties_serial`
    /// into `dir` (the host directory backing `/dev/__properties__`), mode
    /// 0444 as bionic creates them.
    pub fn write_to_dir(&self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        write_readonly(&dir.join(PROPERTY_INFO), &self.info)?;
        for (context, area) in self.contexts.iter().zip(&self.areas) {
            write_readonly(&dir.join(context), &area.bytes())?;
        }
        write_readonly(&dir.join(PROPERTIES_SERIAL), &self.serial.bytes())
    }
}

fn write_readonly(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if path.exists() {
        let mut permissions = fs::metadata(path)?.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions)?;
    }
    fs::write(path, bytes)?;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions)
}
