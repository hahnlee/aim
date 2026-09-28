#![forbid(unsafe_op_in_unsafe_fn)]

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ffi::{c_int, c_void};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock};

pub type Destructor = unsafe extern "C" fn(*mut c_void);
pub type UnregisterAtforkHook = unsafe extern "C" fn(*mut c_void);
pub type StdioCleanupHook = unsafe extern "C" fn();

#[derive(Clone, Copy, Default)]
pub struct Hooks {
    pub unregister_atfork: Option<UnregisterAtforkHook>,
    pub stdio_cleanup: Option<StdioCleanupHook>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Registration {
    pub function: usize,
    pub argument: usize,
    pub dso: usize,
}

#[derive(Clone, Copy)]
struct Entry {
    function: Destructor,
    argument: usize,
    dso: usize,
    sequence: u64,
}

struct PublishedImage {
    start: usize,
    end: usize,
    finalizing: bool,
    handles: HashSet<usize>,
}

#[derive(Default)]
struct State {
    entries: Vec<Option<Entry>>,
    published: HashSet<usize>,
    images: Vec<PublishedImage>,
    active_callbacks: HashMap<usize, usize>,
    thread_destructors: HashMap<usize, usize>,
}

struct ThreadDestructorEntry {
    lifecycle: Arc<Lifecycle>,
    function: Destructor,
    argument: usize,
    dso: usize,
}

#[derive(Default)]
struct ThreadDestructorList(Vec<ThreadDestructorEntry>);

impl Drop for ThreadDestructorList {
    fn drop(&mut self) {
        while let Some(entry) = self.0.pop() {
            // SAFETY: the lifecycle keeps the owning image published until this count drops.
            unsafe { (entry.function)(entry.argument as *mut c_void) };
            entry.lifecycle.complete_thread_destructor(entry.dso);
        }
    }
}

thread_local! {
    static THREAD_DESTRUCTORS: RefCell<ThreadDestructorList> =
        RefCell::new(ThreadDestructorList::default());
}

pub struct Lifecycle {
    state: Mutex<State>,
    quiesced: Condvar,
    hooks: Hooks,
    allow_global: bool,
}

static ACTIVE: OnceLock<RwLock<Vec<Arc<Lifecycle>>>> = OnceLock::new();
static ENTRY_COORDINATOR: Mutex<()> = Mutex::new(());
static NEXT_SEQUENCE: AtomicU64 = AtomicU64::new(1);

fn active_slot() -> &'static RwLock<Vec<Arc<Lifecycle>>> {
    ACTIVE.get_or_init(|| RwLock::new(Vec::new()))
}

impl Lifecycle {
    pub fn new(hooks: Hooks) -> Self {
        Self {
            state: Mutex::new(State::default()),
            quiesced: Condvar::new(),
            hooks,
            allow_global: true,
        }
    }

    fn new_image_owner() -> Self {
        Self {
            state: Mutex::new(State::default()),
            quiesced: Condvar::new(),
            hooks: Hooks::default(),
            allow_global: false,
        }
    }

    pub fn activate(self: &Arc<Self>) -> Result<Activation, &'static str> {
        let mut active = active_slot()
            .write()
            .map_err(|_| "activation lock poisoned")?;
        if active.iter().any(|current| Arc::ptr_eq(current, self)) {
            return Err("DSO lifecycle provider is already active");
        }
        if self.allow_global && active.iter().any(|current| current.allow_global) {
            return Err("process-global DSO lifecycle provider is already active");
        }
        active
            .try_reserve(1)
            .map_err(|_| "could not reserve active lifecycle state")?;
        active.push(Arc::clone(self));
        Ok(Activation {
            lifecycle: Arc::clone(self),
            active: true,
        })
    }

    pub fn publish(&self, dso: usize) -> Result<(), &'static str> {
        if dso == 0 {
            return Err("null is reserved for process-global registrations");
        }
        let mut state = self.state.lock().map_err(|_| "state lock poisoned")?;
        if state.published.contains(&dso) {
            return Err("DSO handle is already published");
        }
        state
            .published
            .try_reserve(1)
            .map_err(|_| "could not reserve published DSO state")?;
        state.published.insert(dso);
        Ok(())
    }

    pub fn try_unpublish(&self, dso: usize) -> Result<(), &'static str> {
        let mut state = self.state.lock().map_err(|_| "state lock poisoned")?;
        if !state.published.contains(&dso) {
            return Err("DSO handle is not published");
        }
        if state.entries.iter().flatten().any(|entry| entry.dso == dso)
            || state.active_callbacks.get(&dso).copied().unwrap_or(0) != 0
        {
            return Err("DSO still has registered or running destructors");
        }
        state.published.remove(&dso);
        state.active_callbacks.remove(&dso);
        Ok(())
    }

    pub fn publish_image(&self, start: usize, end: usize) -> Result<(), &'static str> {
        if start == 0 || start >= end {
            return Err("invalid image address range");
        }
        let mut state = self.state.lock().map_err(|_| "state lock poisoned")?;
        if state
            .images
            .iter()
            .any(|image| start < image.end && image.start < end)
        {
            return Err("image address range overlaps a live image");
        }
        state
            .images
            .try_reserve(1)
            .map_err(|_| "could not reserve image lifecycle state")?;
        state.images.push(PublishedImage {
            start,
            end,
            finalizing: false,
            handles: HashSet::new(),
        });
        Ok(())
    }

    pub fn finalize_image(&self, start: usize, end: usize) -> Result<(), &'static str> {
        let handles = {
            let mut state = self.state.lock().map_err(|_| "state lock poisoned")?;
            let image_index = state
                .images
                .iter()
                .position(|image| image.start == start && image.end == end)
                .ok_or("image address range is not published")?;
            if state.images[image_index].finalizing {
                return Err("image address range is already finalizing");
            }
            let handles = state.images[image_index]
                .handles
                .iter()
                .copied()
                .collect::<HashSet<_>>();
            if handles
                .iter()
                .any(|handle| state.thread_destructors.get(handle).copied().unwrap_or(0) != 0)
            {
                return Err("image still owns live thread-local destructors");
            }
            state.images[image_index].finalizing = true;
            handles
        };

        let mut state = loop {
            self.finalize_matching(|entry| handles.contains(&entry.dso));
            let mut state = self.state.lock().map_err(|_| "state lock poisoned")?;
            while handles
                .iter()
                .any(|handle| state.active_callbacks.get(handle).copied().unwrap_or(0) != 0)
            {
                state = self
                    .quiesced
                    .wait(state)
                    .map_err(|_| "state lock poisoned while quiescing callbacks")?;
            }
            if state
                .entries
                .iter()
                .flatten()
                .any(|entry| handles.contains(&entry.dso))
            {
                drop(state);
                continue;
            }
            break state;
        };
        let index = state
            .images
            .iter()
            .position(|image| image.start == start && image.end == end)
            .ok_or("image address range disappeared during finalization")?;
        state.images.swap_remove(index);
        drop(state);

        if let Some(unregister) = self.hooks.unregister_atfork {
            for handle in handles {
                // SAFETY: the hook has fixed one-pointer PCS and remains live with the provider.
                unsafe { unregister(handle as *mut c_void) };
            }
        }
        Ok(())
    }

    pub fn registrations(&self) -> Result<Vec<Registration>, &'static str> {
        let state = self.state.lock().map_err(|_| "state lock poisoned")?;
        Ok(state
            .entries
            .iter()
            .flatten()
            .map(|entry| Registration {
                function: entry.function as usize,
                argument: entry.argument,
                dso: entry.dso,
            })
            .collect())
    }

    pub fn registration_count(&self, dso: usize) -> Result<usize, &'static str> {
        let state = self.state.lock().map_err(|_| "state lock poisoned")?;
        Ok(state
            .entries
            .iter()
            .flatten()
            .filter(|entry| dso == 0 || entry.dso == dso)
            .count())
    }

    pub fn active_callback_count(&self, dso: usize) -> Result<usize, &'static str> {
        let state = self.state.lock().map_err(|_| "state lock poisoned")?;
        if dso == 0 {
            return Ok(state.active_callbacks.values().sum());
        }
        Ok(state.active_callbacks.get(&dso).copied().unwrap_or(0))
    }

    fn owns_handle(&self, dso: usize) -> bool {
        let Ok(state) = self.state.lock() else {
            std::process::abort();
        };
        state.published.contains(&dso)
            || state
                .images
                .iter()
                .any(|image| image.handles.contains(&dso))
    }

    fn is_quiescent(&self) -> bool {
        let Ok(state) = self.state.lock() else {
            std::process::abort();
        };
        state.entries.iter().all(Option::is_none)
            && state.images.is_empty()
            && state.published.is_empty()
            && state.active_callbacks.is_empty()
            && state.thread_destructors.is_empty()
    }

    fn register(
        &self,
        function: Option<Destructor>,
        argument: *mut c_void,
        dso: *mut c_void,
    ) -> c_int {
        let Some(function) = function else {
            return -1;
        };
        let mut dso = dso as usize;
        let Ok(_coordinator) = ENTRY_COORDINATOR.lock() else {
            return -1;
        };
        let Ok(mut state) = self.state.lock() else {
            return -1;
        };
        // Some Android C++ runtimes pass a null DSO cookie to
        // __cxa_atexit.  The destructor code pointer still identifies the
        // owning ELF image, so bind that registration to a synthetic,
        // image-local cookie.  This keeps teardown safe and prevents a
        // process-global null entry from surviving an APK unload.
        if dso == 0 {
            let function_address = function as usize;
            let argument_address = argument as usize;
            if let Some(image) = state.images.iter_mut().find(|image| {
                (function_address >= image.start && function_address < image.end)
                    || (argument_address >= image.start && argument_address < image.end)
            }) {
                // Use the image start as a stable synthetic cookie. It is
                // guaranteed non-zero and remains inside the exact image
                // envelope used by finalization.
                dso = image.start;
                image.handles.insert(dso);
            }
        }
        if dso != 0 && !state.published.contains(&dso) {
            let Some(image_index) = state
                .images
                .iter()
                .position(|image| dso >= image.start && dso < image.end)
            else {
                return -1;
            };
            let can_admit = !state.images[image_index].finalizing;
            let is_reentrant = state.images[image_index].handles.contains(&dso)
                && state.active_callbacks.get(&dso).copied().unwrap_or(0) != 0;
            if !can_admit && !is_reentrant {
                return -1;
            }
            if can_admit && !state.images[image_index].handles.contains(&dso) {
                if state.images[image_index].handles.try_reserve(1).is_err() {
                    return -1;
                }
                state.images[image_index].handles.insert(dso);
            }
        }
        if state.entries.try_reserve(1).is_err() {
            return -1;
        }
        let Ok(sequence) =
            NEXT_SEQUENCE.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                next.checked_add(1)
            })
        else {
            return -1;
        };
        state.entries.push(Some(Entry {
            function,
            argument: argument as usize,
            dso,
            sequence,
        }));
        0
    }

    fn register_thread(
        self: &Arc<Self>,
        function: Option<Destructor>,
        argument: *mut c_void,
        dso: *mut c_void,
    ) -> c_int {
        let Some(function) = function else { return -1 };
        let dso = dso as usize;
        if dso == 0 {
            return -1;
        }
        {
            let Ok(mut state) = self.state.lock() else {
                return -1;
            };
            if !state.published.contains(&dso) {
                let Some(image_index) = state
                    .images
                    .iter()
                    .position(|image| dso >= image.start && dso < image.end)
                else {
                    return -1;
                };
                if state.images[image_index].finalizing {
                    return -1;
                }
                if !state.images[image_index].handles.contains(&dso) {
                    if state.images[image_index].handles.try_reserve(1).is_err() {
                        return -1;
                    }
                    state.images[image_index].handles.insert(dso);
                }
            }
            *state.thread_destructors.entry(dso).or_insert(0) += 1;
        }
        let entry = ThreadDestructorEntry {
            lifecycle: Arc::clone(self),
            function,
            argument: argument as usize,
            dso,
        };
        let inserted = THREAD_DESTRUCTORS
            .try_with(|entries| {
                let mut entries = entries.borrow_mut();
                if entries.0.try_reserve(1).is_err() {
                    return false;
                }
                entries.0.push(entry);
                true
            })
            .unwrap_or(false);
        if !inserted {
            self.complete_thread_destructor(dso);
            return -1;
        }
        0
    }

    fn complete_thread_destructor(&self, dso: usize) {
        let Ok(mut state) = self.state.lock() else {
            std::process::abort();
        };
        let Some(count) = state.thread_destructors.get_mut(&dso) else {
            std::process::abort();
        };
        *count -= 1;
        if *count == 0 {
            state.thread_destructors.remove(&dso);
            self.quiesced.notify_all();
        }
    }

    fn finalize(&self, dso: *mut c_void) {
        let dso = dso as usize;
        self.finalize_matching(|entry| dso == 0 || entry.dso == dso);

        if dso == 0 {
            if let Some(cleanup) = self.hooks.stdio_cleanup {
                // SAFETY: the hook has fixed no-argument PCS and remains live with the provider.
                unsafe { cleanup() };
            }
        } else if let Some(unregister) = self.hooks.unregister_atfork {
            // SAFETY: the hook has fixed one-pointer PCS and remains live with the provider.
            unsafe { unregister(dso as *mut c_void) };
        }
    }

    fn finalize_matching(&self, matches: impl Fn(Entry) -> bool) {
        loop {
            let Some(entry) = self.take_latest_matching(&matches) else {
                break;
            };

            // SAFETY: the loader published the DSO while its executable mapping was alive.
            // The one-pointer callback uses x0 in both Android and Darwin arm64 PCS.
            unsafe { (entry.function)(entry.argument as *mut c_void) };

            self.complete_callback(entry.dso);
        }

        let Ok(_coordinator) = ENTRY_COORDINATOR.lock() else {
            std::process::abort();
        };
        let Ok(mut state) = self.state.lock() else {
            std::process::abort();
        };
        state.entries.retain(Option::is_some);
    }

    fn take_latest_matching(&self, matches: &impl Fn(Entry) -> bool) -> Option<Entry> {
        let Ok(_coordinator) = ENTRY_COORDINATOR.lock() else {
            std::process::abort();
        };
        let Ok(mut state) = self.state.lock() else {
            std::process::abort();
        };
        let index = state
            .entries
            .iter()
            .rposition(|slot| slot.is_some_and(matches))?;
        let entry = state.entries[index]
            .take()
            .expect("selected lifecycle entry must be present");
        *state.active_callbacks.entry(entry.dso).or_insert(0) += 1;
        Some(entry)
    }

    fn complete_callback(&self, dso: usize) {
        let Ok(mut state) = self.state.lock() else {
            std::process::abort();
        };
        let Some(active) = state.active_callbacks.get_mut(&dso) else {
            std::process::abort();
        };
        *active -= 1;
        if *active == 0 {
            state.active_callbacks.remove(&dso);
            self.quiesced.notify_all();
        }
    }
}

pub struct Activation {
    lifecycle: Arc<Lifecycle>,
    active: bool,
}

impl Drop for Activation {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let Ok(mut active) = active_slot().write() else {
            std::process::abort();
        };
        active.retain(|current| !Arc::ptr_eq(current, &self.lifecycle));
        self.active = false;
    }
}

fn active_lifecycles() -> Vec<Arc<Lifecycle>> {
    let Ok(active) = active_slot().read() else {
        std::process::abort();
    };
    active.clone()
}

fn take_latest_global() -> Option<(Arc<Lifecycle>, Entry)> {
    let Ok(_coordinator) = ENTRY_COORDINATOR.lock() else {
        std::process::abort();
    };
    let lifecycles = active_lifecycles();
    let mut selected: Option<(Arc<Lifecycle>, usize, u64)> = None;
    for lifecycle in lifecycles {
        let Ok(state) = lifecycle.state.lock() else {
            std::process::abort();
        };
        for (index, entry) in state.entries.iter().enumerate() {
            let Some(entry) = entry else { continue };
            if selected
                .as_ref()
                .is_none_or(|(_, _, sequence)| entry.sequence > *sequence)
            {
                selected = Some((Arc::clone(&lifecycle), index, entry.sequence));
            }
        }
    }
    let (lifecycle, index, sequence) = selected?;
    let entry = {
        let Ok(mut state) = lifecycle.state.lock() else {
            std::process::abort();
        };
        let entry = state.entries[index]
            .take()
            .expect("globally selected lifecycle entry must be present");
        assert_eq!(entry.sequence, sequence);
        *state.active_callbacks.entry(entry.dso).or_insert(0) += 1;
        entry
    };
    Some((lifecycle, entry))
}

fn finalize_all_active() {
    while let Some((lifecycle, entry)) = take_latest_global() {
        // SAFETY: the owning loader mapping remains published while its callback is active.
        unsafe { (entry.function)(entry.argument as *mut c_void) };
        lifecycle.complete_callback(entry.dso);
    }

    if let Some(cleanup) = active_lifecycles()
        .into_iter()
        .find(|lifecycle| lifecycle.allow_global)
        .and_then(|lifecycle| lifecycle.hooks.stdio_cleanup)
    {
        // SAFETY: the provider-owned hook has fixed no-argument PCS and process lifetime.
        unsafe { cleanup() };
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn aim_bionic_dso_cxa_atexit_core(
    function: Option<Destructor>,
    argument: *mut c_void,
    dso: *mut c_void,
) -> c_int {
    if dso.is_null() {
        return active_lifecycles()
            .into_iter()
            .find(|lifecycle| lifecycle.allow_global)
            .map_or(-1, |lifecycle| lifecycle.register(function, argument, dso));
    }
    for lifecycle in active_lifecycles() {
        if lifecycle.register(function, argument, dso) == 0 {
            return 0;
        }
    }
    -1
}

#[unsafe(no_mangle)]
pub extern "C" fn aim_bionic_dso_cxa_finalize_core(dso: *mut c_void) {
    if dso.is_null() {
        finalize_all_active();
        return;
    }
    for lifecycle in active_lifecycles() {
        if lifecycle.owns_handle(dso as usize) {
            lifecycle.finalize(dso);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn aim_bionic_dso_cxa_thread_atexit_core(
    function: Option<Destructor>,
    argument: *mut c_void,
    dso: *mut c_void,
) -> c_int {
    for lifecycle in active_lifecycles() {
        if lifecycle.register_thread(function, argument, dso) == 0 {
            return 0;
        }
    }
    -1
}

pub struct AimBionicDsoLifecycleOwner {
    lifecycle: Arc<Lifecycle>,
    _activation: Activation,
}

#[unsafe(no_mangle)]
pub extern "C" fn aim_bionic_dso_lifecycle_owner_create() -> *mut AimBionicDsoLifecycleOwner {
    let lifecycle = Arc::new(Lifecycle::new_image_owner());
    let Ok(activation) = lifecycle.activate() else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(AimBionicDsoLifecycleOwner {
        lifecycle,
        _activation: activation,
    }))
}

#[unsafe(no_mangle)]
/// # Safety
/// `owner` must be null or the unique pointer returned by `owner_create`, and may be destroyed
/// exactly once after all loader callbacks have quiesced.
pub unsafe extern "C" fn aim_bionic_dso_lifecycle_owner_destroy(
    owner: *mut AimBionicDsoLifecycleOwner,
) {
    if !owner.is_null() {
        // SAFETY: validated by the function contract.
        let owner_ref = unsafe { &*owner };
        if !owner_ref.lifecycle.is_quiescent() {
            std::process::abort();
        }
        // SAFETY: ownership is returned exactly once by the matching create function.
        drop(unsafe { Box::from_raw(owner) });
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// `context` must be a live owner pointer retained until the matching image finalization call.
pub unsafe extern "C" fn aim_bionic_dso_lifecycle_publish_image(
    context: *mut c_void,
    start: usize,
    end: usize,
) -> c_int {
    // SAFETY: the loader retains the owner context through the graph callback lifetime.
    let Some(owner) = (unsafe { (context as *mut AimBionicDsoLifecycleOwner).as_ref() }) else {
        return -1;
    };
    owner.lifecycle.publish_image(start, end).map_or(-1, |()| 0)
}

#[unsafe(no_mangle)]
/// # Safety
/// `context` must be the owner that published this exact still-mapped address range.
pub unsafe extern "C" fn aim_bionic_dso_lifecycle_finalize_image(
    context: *mut c_void,
    start: usize,
    end: usize,
) -> c_int {
    // SAFETY: the loader retains the owner context through the graph callback lifetime.
    let Some(owner) = (unsafe { (context as *mut AimBionicDsoLifecycleOwner).as_ref() }) else {
        return -1;
    };
    owner
        .lifecycle
        .finalize_image(start, end)
        .map_or(-1, |()| 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static LOG: Mutex<Vec<usize>> = Mutex::new(Vec::new());
    static TEST_SERIAL: Mutex<()> = Mutex::new(());
    static ARGUMENT_ONE: u8 = 1;
    static ARGUMENT_TWO: u8 = 2;

    unsafe extern "C" fn record(argument: *mut c_void) {
        LOG.lock().unwrap().push(argument as usize);
    }

    #[test]
    fn routes_concurrent_image_owners_and_rejects_null_registrations() {
        let _serial = TEST_SERIAL.lock().unwrap();
        LOG.lock().unwrap().clear();
        let first = Arc::new(Lifecycle::new_image_owner());
        let second = Arc::new(Lifecycle::new_image_owner());
        let first_activation = first.activate().unwrap();
        let second_activation = second.activate().unwrap();
        first.publish_image(0x1000, 0x2000).unwrap();
        second.publish_image(0x3000, 0x4000).unwrap();

        assert_eq!(
            aim_bionic_dso_cxa_atexit_core(
                Some(record),
                std::ptr::from_ref(&ARGUMENT_ONE).cast_mut().cast(),
                0x1100_usize as *mut c_void,
            ),
            0
        );
        assert_eq!(
            aim_bionic_dso_cxa_atexit_core(
                Some(record),
                std::ptr::from_ref(&ARGUMENT_TWO).cast_mut().cast(),
                0x3100_usize as *mut c_void,
            ),
            0
        );
        assert_eq!(
            aim_bionic_dso_cxa_atexit_core(
                Some(record),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ),
            -1
        );

        first.finalize_image(0x1000, 0x2000).unwrap();
        assert_eq!(
            *LOG.lock().unwrap(),
            [std::ptr::from_ref(&ARGUMENT_ONE) as usize]
        );
        second.finalize_image(0x3000, 0x4000).unwrap();
        assert_eq!(
            *LOG.lock().unwrap(),
            [
                std::ptr::from_ref(&ARGUMENT_ONE) as usize,
                std::ptr::from_ref(&ARGUMENT_TWO) as usize,
            ]
        );
        assert!(first.is_quiescent());
        assert!(second.is_quiescent());
        drop(second_activation);
        drop(first_activation);
    }

    #[test]
    fn runs_thread_local_destructors_in_reverse_order_on_thread_exit() {
        let _serial = TEST_SERIAL.lock().unwrap();
        LOG.lock().unwrap().clear();
        let lifecycle = Arc::new(Lifecycle::new_image_owner());
        let _activation = lifecycle.activate().unwrap();
        lifecycle.publish_image(0x1000, 0x2000).unwrap();
        std::thread::spawn(|| {
            assert_eq!(
                aim_bionic_dso_cxa_thread_atexit_core(
                    Some(record),
                    std::ptr::from_ref(&ARGUMENT_ONE).cast_mut().cast(),
                    0x1100_usize as *mut c_void,
                ),
                0
            );
            assert_eq!(
                aim_bionic_dso_cxa_thread_atexit_core(
                    Some(record),
                    std::ptr::from_ref(&ARGUMENT_TWO).cast_mut().cast(),
                    0x1100_usize as *mut c_void,
                ),
                0
            );
        })
        .join()
        .unwrap();
        assert_eq!(
            *LOG.lock().unwrap(),
            vec![
                std::ptr::from_ref(&ARGUMENT_TWO) as usize,
                std::ptr::from_ref(&ARGUMENT_ONE) as usize,
            ]
        );
        lifecycle.finalize_image(0x1000, 0x2000).unwrap();
    }

    #[test]
    fn null_finalize_drains_interleaved_image_owners_in_global_lifo_order() {
        let _serial = TEST_SERIAL.lock().unwrap();
        LOG.lock().unwrap().clear();
        let first = Arc::new(Lifecycle::new_image_owner());
        let second = Arc::new(Lifecycle::new_image_owner());
        let first_activation = first.activate().unwrap();
        let second_activation = second.activate().unwrap();
        first.publish_image(0x1000, 0x2000).unwrap();
        second.publish_image(0x3000, 0x4000).unwrap();

        for (argument, dso) in [(1, 0x1100), (2, 0x3100), (3, 0x1100), (4, 0x3100)] {
            assert_eq!(
                aim_bionic_dso_cxa_atexit_core(
                    Some(record),
                    argument as *mut c_void,
                    dso as *mut c_void,
                ),
                0
            );
        }
        assert_eq!(
            aim_bionic_dso_cxa_atexit_core(
                Some(record),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ),
            -1
        );

        aim_bionic_dso_cxa_finalize_core(std::ptr::null_mut());
        assert_eq!(*LOG.lock().unwrap(), [4, 3, 2, 1]);
        assert_eq!(first.registration_count(0).unwrap(), 0);
        assert_eq!(second.registration_count(0).unwrap(), 0);

        first.finalize_image(0x1000, 0x2000).unwrap();
        second.finalize_image(0x3000, 0x4000).unwrap();
        assert!(first.is_quiescent());
        assert!(second.is_quiescent());
        drop(second_activation);
        drop(first_activation);
    }

    #[test]
    fn permits_multiple_image_owners_but_only_one_global_owner() {
        let _serial = TEST_SERIAL.lock().unwrap();
        let first = Arc::new(Lifecycle::new(Hooks::default()));
        let second = Arc::new(Lifecycle::new(Hooks::default()));
        let activation = first.activate().unwrap();
        assert!(second.activate().is_err());
        drop(activation);
        let second_activation = second.activate().unwrap();
        drop(second_activation);
    }
}
