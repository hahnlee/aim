//! A provider's cursor across binder, as `ContentProviderNative` sends one
//! at `android-16.0.0_r1`: `CursorToBulkCursorAdaptor` over a
//! `MatrixCursor` of strings, served as `IBulkCursor` (`BulkCursorNative`)
//! and described by its `BulkCursorDescriptor`, and the `CursorWindow`s
//! its `getWindow` fills (`DatabaseUtils.cursorFillWindow`) as
//! `libs/androidfw/CursorWindow.cpp` lays them out: the values' UTF-8 from
//! the start, a 16-byte field slot per value from the end, sent inline
//! while the window fits in 16 KiB and as an ashmem region of the
//! configured window size once it grew past that (`maybeInflate`).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::os::fd::AsFd;
use std::os::unix::fs::FileExt;
use std::sync::{Arc, Mutex, Weak};

use aim_binder_host::local::{Call, LocalProcess, Reply, Service, Strong};
use aim_binder_host::parcel::{BAD_VALUE, Binder, EX_NULL_POINTER, Exception, Parcel, UNKNOWN_TRANSACTION};

use crate::bundle;

/// `IBulkCursor.descriptor` and its transactions, which the framework
/// writes by hand in an interface whose descriptor is not its class's.
const DESCRIPTOR: &str = "android.content.IBulkCursor";
const GET_CURSOR_WINDOW_TRANSACTION: u32 = 1;
const DEACTIVATE_TRANSACTION: u32 = 2;
const REQUERY_TRANSACTION: u32 = 3;
const ON_MOVE_TRANSACTION: u32 = 4;
const GET_EXTRAS_TRANSACTION: u32 = 5;
const RESPOND_TRANSACTION: u32 = 6;
const CLOSE_TRANSACTION: u32 = 7;
/// `CursorWindow.cpp`'s `kInlineSize` and `kSlotSizeBytes`.
const INLINE_SIZE: usize = 16384;
const SLOT_SIZE: usize = 16;
/// `CursorWindow::FIELD_TYPE_STRING`.
const FIELD_TYPE_STRING: i32 = 3;

/// A `MatrixCursor` whose values are strings or null.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rows {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
}

/// `CursorWindow`, as its native side keeps it.
struct Window {
    name: String,
    size: usize,
    inflated_size: usize,
    start: i32,
    columns: usize,
    rows: usize,
    heap: Vec<u8>,
    /// The field slots, by index (`row * columns + column`).
    slots: Vec<[u8; SLOT_SIZE]>,
    /// The ashmem region of an inflated window.
    region: Option<std::fs::File>,
}

impl Window {
    /// `CursorWindow::create`: inline, inflated up to `inflated_size`.
    fn new(name: &str, inflated_size: usize) -> Window {
        Window {
            name: name.to_string(),
            size: INLINE_SIZE.min(inflated_size),
            inflated_size,
            start: 0,
            columns: 0,
            rows: 0,
            heap: Vec::new(),
            slots: Vec::new(),
            region: None,
        }
    }

    /// `mSlotsOffset`.
    fn slots_offset(&self) -> usize {
        self.size - self.slots.len() * SLOT_SIZE
    }

    fn clear(&mut self) {
        self.heap.clear();
        self.slots.clear();
        self.rows = 0;
        self.columns = 0;
    }

    /// `maybeInflate`: the window grown to its inflated size, in an ashmem
    /// region; whether it grew.
    fn inflate(&mut self) -> bool {
        if self.size == self.inflated_size {
            return false;
        }
        let region = aim_ashmem::read_only_region(format!("CursorWindow: {}", self.name).as_bytes())
            .and_then(|f| f.set_len(self.inflated_size as u64).map(|()| f));
        match region {
            Ok(region) => {
                self.region = Some(region);
                self.size = self.inflated_size;
                true
            }
            Err(e) => {
                eprintln!("app host: cursor window: Failed ashmem_create_region: {e}");
                false
            }
        }
    }

    /// `allocRow`: a row of null fields.
    fn alloc_row(&mut self) -> bool {
        let fits = |w: &Window| w.slots_offset() >= w.heap.len() + w.columns * SLOT_SIZE;
        if !(fits(self) || self.inflate() && fits(self)) {
            return false;
        }
        self.slots.extend(std::iter::repeat_n([0; SLOT_SIZE], self.columns));
        self.rows += 1;
        true
    }

    fn free_last_row(&mut self) {
        self.slots.truncate(self.slots.len() - self.columns);
        self.rows -= 1;
    }

    /// `putString` of `value`, its terminating NUL included.
    fn put_string(&mut self, row: usize, column: usize, value: &[u8]) -> bool {
        let index = row * self.columns + column;
        if column >= self.columns || index >= self.slots.len() {
            return false;
        }
        // `alloc`: 4-byte aligned, below the slots.
        let aligned = (value.len() + 3) & !3;
        let fits = |w: &Window| w.heap.len() + aligned <= w.slots_offset();
        if !(fits(self) || self.inflate() && fits(self)) {
            return false;
        }
        let offset = self.heap.len();
        self.heap.extend_from_slice(value);
        self.heap.resize(offset + aligned, 0);
        let slot = &mut self.slots[index];
        slot[0..4].copy_from_slice(&FIELD_TYPE_STRING.to_le_bytes());
        slot[4..8].copy_from_slice(&(offset as u32).to_le_bytes());
        slot[8..12].copy_from_slice(&(value.len() as u32).to_le_bytes());
        true
    }

    /// `DatabaseUtils.cursorFillWindow`: the rows from `position` on, as
    /// many as fit.
    fn fill(&mut self, cursor: &Rows, position: usize) {
        if position >= cursor.rows.len() {
            return;
        }
        self.clear();
        self.start = position as i32;
        self.columns = cursor.columns.len();
        for (row, values) in cursor.rows[position..].iter().enumerate() {
            if !self.alloc_row() {
                break;
            }
            // `putNull` leaves the slot `allocRow` zeroed.
            let put = |w: &mut Window, (column, value): (usize, &Option<String>)| {
                value.as_ref().is_none_or(|v| w.put_string(row, column, &jni_utf(v)))
            };
            if !values.iter().enumerate().all(|v| put(self, v)) {
                self.free_last_row();
                break;
            }
        }
    }

    /// `CursorWindow.writeToParcel`: the start position, then the native
    /// window: inline and compacted, or its ashmem region.
    fn write(&self, p: &mut Parcel) -> Result<(), Exception> {
        p.write_i32(self.start);
        p.write_string8(Some(&self.name));
        p.write_u32(self.rows as u32);
        p.write_u32(self.columns as u32);
        // The slots as laid out in memory: the last one first.
        let slots: Vec<u8> = self.slots.iter().rev().flatten().copied().collect();
        match &self.region {
            Some(region) => {
                let written = region
                    .write_all_at(&self.heap, 0)
                    .and_then(|()| region.write_all_at(&slots, self.slots_offset() as u64));
                written.map_err(|e| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("cursor window: {e}")))?;
                let file = aim_binder_host::server::file_from_fd(region.as_fd())
                    .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "cursor window: no fileport"))?;
                p.write_u32(self.size as u32);
                p.write_bool(true);
                p.write_file(file);
            }
            None => {
                p.write_u32((self.heap.len() + slots.len()) as u32);
                p.write_bool(false);
                p.write_raw(&self.heap, &[]);
                p.write_raw(&slots, &[]);
            }
        }
        Ok(())
    }
}

/// A string as `GetStringUTFChars` hands it to `nativePutString`: ART's
/// modified UTF-8 (a NUL as two bytes, a supplementary character as
/// four), NUL-terminated.
fn jni_utf(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() + 1);
    for part in s.split('\0').enumerate() {
        if part.0 > 0 {
            out.extend_from_slice(&[0xc0, 0x80]);
        }
        out.extend_from_slice(part.1.as_bytes());
    }
    out.push(0);
    out
}

/// `CursorToBulkCursorAdaptor`'s state.
#[derive(Default)]
struct Adaptor {
    /// The cursor, none once closed.
    cursor: Option<Rows>,
    /// `mFilledWindow`.
    filled: Option<Window>,
    /// The client's observer (`ContentObserverProxy`) and the cookie of
    /// its death link.
    observer: Option<(Strong, u64)>,
}

/// `CursorToBulkCursorAdaptor`.
pub struct BulkCursor {
    process: Arc<LocalProcess>,
    this: Weak<BulkCursor>,
    /// `mProviderName`, which names its windows, and the windows' size
    /// (`config_cursorWindowSize`).
    provider: String,
    window_size: usize,
    state: Mutex<Adaptor>,
}

impl BulkCursor {
    /// A cursor sent to a client with its `observer`.
    pub fn new(
        process: Arc<LocalProcess>,
        cursor: Rows,
        observer: Option<Binder>,
        provider: &str,
        window_size: usize,
    ) -> Result<Arc<BulkCursor>, Exception> {
        let adaptor = Arc::new_cyclic(|this| BulkCursor {
            process,
            this: this.clone(),
            provider: provider.to_string(),
            window_size,
            state: Mutex::new(Adaptor { cursor: Some(cursor), ..Default::default() }),
        });
        adaptor.register_observer(&mut adaptor.state.lock().unwrap(), observer)?;
        Ok(adaptor)
    }

    /// `BulkCursorDescriptor.writeToParcel` of `getBulkCursorDescriptor`,
    /// with `binder`, this adaptor served: a `MatrixCursor` has no window
    /// of its own and wants no move calls.
    pub fn write_descriptor(&self, binder: Binder, p: &mut Parcel) -> Result<(), i32> {
        let state = self.state.lock().unwrap();
        let Some(cursor) = &state.cursor else { return closed(&state) };
        p.write_binder(Some(binder));
        p.write_i32(cursor.columns.len() as i32);
        for column in &cursor.columns {
            p.write_string16(Some(column));
        }
        p.write_i32(0);
        p.write_i32(cursor.rows.len() as i32);
        p.write_i32(0);
        Ok(())
    }

    /// `createAndRegisterObserverProxyLocked`: the observer is linked to
    /// death, which disposes of the cursor (`binderDied`).
    fn register_observer(&self, state: &mut Adaptor, observer: Option<Binder>) -> Result<(), Exception> {
        match observer {
            None => Err(Exception::new(EX_NULL_POINTER, "Attempt to invoke interface method 'android.os.IBinder android.database.IContentObserver.asBinder()' on a null object reference")),
            Some(Binder::Handle(handle)) => {
                let strong = self.process.strong(handle);
                let this = self.this.clone();
                let cookie = self.process.link_to_death(
                    &strong,
                    Box::new(move || {
                        if let Some(this) = this.upgrade() {
                            this.dispose(&mut this.state.lock().unwrap());
                        }
                    }),
                );
                state.observer = Some((strong, cookie));
                Ok(())
            }
            // One of the app host's own never dies.
            Some(Binder::Local(_)) => Ok(()),
        }
    }

    fn unregister_observer(&self, state: &mut Adaptor) {
        if let Some((strong, cookie)) = state.observer.take() {
            self.process.clear_death(&strong, cookie);
        }
    }

    /// `disposeLocked`.
    fn dispose(&self, state: &mut Adaptor) {
        if state.cursor.take().is_some() {
            self.unregister_observer(state);
        }
        state.filled = None;
    }

    /// `getWindow`: the window holding `position`, none past the end.
    fn window<'a>(&self, state: &'a mut Adaptor, position: i32) -> Option<&'a Window> {
        let cursor = state.cursor.as_ref()?;
        // `moveToPosition`.
        let Some(position) = usize::try_from(position).ok().filter(|&p| p < cursor.rows.len()) else {
            state.filled = None;
            return None;
        };
        let window = state.filled.get_or_insert_with(|| Window::new(&self.provider, self.window_size));
        window.fill(cursor, position);
        Some(window)
    }
}

/// `throwIfCursorIsClosed`: a `StaleDataException`, which no parcel
/// carries, so the call fails.
fn closed(state: &Adaptor) -> Result<(), i32> {
    if state.cursor.is_none() {
        eprintln!("app host: cursor: Attempted to access a cursor after it has been closed.");
        return Err(UNKNOWN_TRANSACTION);
    }
    Ok(())
}

impl Service for BulkCursor {
    fn descriptor(&self) -> &str {
        DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let r = &mut call.data;
        r.enforce_interface(DESCRIPTOR).map_err(|_| BAD_VALUE)?;
        let mut p = Parcel::new();
        let mut state = self.state.lock().unwrap();
        match call.code {
            GET_CURSOR_WINDOW_TRANSACTION => {
                let position = r.read_i32().map_err(|_| BAD_VALUE)?;
                closed(&state)?;
                match self.window(&mut state, position) {
                    Some(window) => {
                        p.write_no_exception();
                        p.write_i32(1);
                        if let Err(e) = window.write(&mut p) {
                            return super::reply(Err(e));
                        }
                    }
                    None => {
                        p.write_no_exception();
                        p.write_i32(0);
                    }
                }
            }
            DEACTIVATE_TRANSACTION => {
                if state.cursor.is_some() {
                    self.unregister_observer(&mut state);
                }
                state.filled = None;
                p.write_no_exception();
            }
            CLOSE_TRANSACTION => {
                self.dispose(&mut state);
                p.write_no_exception();
            }
            REQUERY_TRANSACTION => {
                let observer = r.read_binder().map_err(|_| BAD_VALUE)?;
                closed(&state)?;
                state.filled = None;
                self.unregister_observer(&mut state);
                if let Err(e) = self.register_observer(&mut state, observer) {
                    return super::reply(Err(e));
                }
                p.write_no_exception();
                p.write_i32(state.cursor.as_ref().map_or(0, |c| c.rows.len() as i32));
                bundle::write(&mut p, &[]);
            }
            ON_MOVE_TRANSACTION => {
                r.read_i32().map_err(|_| BAD_VALUE)?;
                closed(&state)?;
                p.write_no_exception();
            }
            GET_EXTRAS_TRANSACTION => {
                p.write_no_exception();
                bundle::write(&mut p, &[]);
            }
            RESPOND_TRANSACTION => {
                bundle::read(r).map_err(|_| BAD_VALUE)?;
                p.write_no_exception();
                bundle::write(&mut p, &[]);
            }
            _ => return Err(UNKNOWN_TRANSACTION),
        }
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::Reader;

    fn rows(values: &[&[Option<&str>]]) -> Rows {
        Rows {
            columns: vec!["name".into(), "value".into()],
            rows: values.iter().map(|r| r.iter().map(|v| v.map(str::to_string)).collect()).collect(),
        }
    }

    /// Each row's fields: their types and bytes.
    type Fields = Vec<Vec<(i32, Vec<u8>)>>;

    /// A window read as `CursorWindow::createFromParcel` and
    /// `getFieldSlot` read it.
    fn read_inline(p: &Parcel) -> (i32, String, Fields) {
        let mut r = Reader::new(p.data(), p.objects());
        let start = r.read_i32().unwrap();
        let name = r.read_string8().unwrap().unwrap();
        let (rows, columns) = (r.read_u32().unwrap() as usize, r.read_u32().unwrap() as usize);
        let size = r.read_u32().unwrap() as usize;
        assert!(!r.read_bool().unwrap());
        let at = r.position();
        let data = &p.data()[at..at + size];
        let fields = (0..rows)
            .map(|row| {
                (0..columns)
                    .map(|column| {
                        let slot = size - SLOT_SIZE - (row * columns + column) * SLOT_SIZE;
                        let int = |o: usize| u32::from_le_bytes(data[slot + o..slot + o + 4].try_into().unwrap());
                        let (offset, len) = (int(4) as usize, int(8) as usize);
                        (int(0) as i32, data[offset..offset + len].to_vec())
                    })
                    .collect()
            })
            .collect();
        (start, name, fields)
    }

    #[test]
    fn a_small_window_is_inline_and_compacted() {
        let cursor = rows(&[&[Some("a"), Some("1")], &[Some("b"), None]]);
        let mut window = Window::new("P", 2 * 1024 * 1024);
        window.fill(&cursor, 0);
        let mut p = Parcel::new();
        window.write(&mut p).unwrap();
        let (start, name, fields) = read_inline(&p);
        assert_eq!((start, name.as_str()), (0, "P"));
        assert_eq!(fields[0], [(3, b"a\0".to_vec()), (3, b"1\0".to_vec())]);
        assert_eq!(fields[1], [(3, b"b\0".to_vec()), (0, Vec::new())]);
        // Two strings of 4 aligned bytes each, then 3 used slots and a null.
        assert_eq!(p.data().len(), 4 + 8 + 8 + 4 + 4 + 12 + 4 * SLOT_SIZE);
    }

    #[test]
    fn a_window_starts_at_its_position() {
        let cursor = rows(&[&[Some("a"), None], &[Some("b"), Some("2")]]);
        let mut window = Window::new("P", 1 << 20);
        window.fill(&cursor, 1);
        let mut p = Parcel::new();
        window.write(&mut p).unwrap();
        let (start, _, fields) = read_inline(&p);
        assert_eq!(start, 1);
        assert_eq!(fields, [[(3, b"b\0".to_vec()), (3, b"2\0".to_vec())]]);
    }

    #[test]
    fn a_window_inflates_then_stops_at_its_size() {
        let value = "v".repeat(1000);
        let cursor = Rows { columns: vec!["value".into()], rows: vec![vec![Some(value)]; 40] };
        // 1004 bytes and a slot a row: 16 fit inline, 24 in 24 KiB.
        let mut window = Window::new("P", 24 * 1024);
        window.fill(&cursor, 0);
        assert_eq!((window.rows, window.size), (24, 24 * 1024));
        assert!(window.region.is_some());
        let mut small = Window::new("P", 8 * 1024);
        small.fill(&cursor, 0);
        assert_eq!((small.rows, small.size), (8, 8 * 1024));
        assert!(small.region.is_none());
    }

    /// `BulkCursorDescriptor`, then `getWindow` through `BulkCursorProxy`:
    /// a window of the rows from the position, none past the end, and a
    /// failed call once closed.
    #[test]
    fn the_adaptor_serves_its_windows() {
        let driver = aim_binder_driver::Driver::new();
        let credentials = aim_binder_driver::Credentials { pid: 97_201, euid: 1000, security_context: None };
        let process = LocalProcess::open(&driver, aim_binder_driver::Device::Binder, credentials);
        let cursor = rows(&[&[Some("a"), Some("1")], &[Some("b"), Some("2")]]);
        let adaptor = BulkCursor::new(process, cursor, Some(Binder::Local(0x10)), "P", 1 << 20).unwrap();
        let mut p = Parcel::new();
        adaptor.write_descriptor(Binder::Local(0x20), &mut p).unwrap();
        let mut r = Reader::new(p.data(), p.objects());
        assert_eq!(r.read_binder(), Ok(Some(Binder::Local(0x20))));
        assert_eq!(r.read_i32(), Ok(2));
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("name"));
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("value"));
        // No move calls, two rows, no window of its own.
        assert_eq!((r.read_i32(), r.read_i32(), r.read_i32()), (Ok(0), Ok(2), Ok(0)));
        let window = |code, position: i32| {
            let mut data = Parcel::new();
            data.write_interface_token(DESCRIPTOR);
            data.write_i32(position);
            let mut call = Call { code, flags: 0, sender_pid: 1, sender_euid: 10_050, data: Reader::new(data.data(), &[]) };
            adaptor.transact(&mut call)
        };
        let reply = window(GET_CURSOR_WINDOW_TRANSACTION, 1).unwrap();
        let mut r = Reader::new(reply.data(), reply.objects());
        assert_eq!((r.read_i32(), r.read_i32()), (Ok(0), Ok(1)));
        let mut sent = Parcel::new();
        sent.write_raw(&reply.data()[r.position()..], &[]);
        let (start, name, fields) = read_inline(&sent);
        assert_eq!((start, name.as_str()), (1, "P"));
        assert_eq!(fields, [[(3, b"b\0".to_vec()), (3, b"2\0".to_vec())]]);
        let reply = window(GET_CURSOR_WINDOW_TRANSACTION, 2).unwrap();
        assert_eq!(&reply.data()[reply.data().len() - 8..], [0; 8]);
        assert!(window(CLOSE_TRANSACTION, 0).is_ok());
        assert_eq!(window(GET_CURSOR_WINDOW_TRANSACTION, 0).err(), Some(UNKNOWN_TRANSACTION));
    }

    #[test]
    fn strings_are_jni_modified_utf8() {
        assert_eq!(jni_utf("a\0b"), b"a\xc0\x80b\0");
        assert_eq!(jni_utf("\u{1f600}"), "\u{1f600}\0".as_bytes());
    }
}
