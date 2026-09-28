use std::alloc::{Layout, alloc_zeroed, dealloc, handle_alloc_error};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use super::{LoadError, ProgramHeader, checked_slice, to_usize};

pub(crate) const MAX_TLS_SIZE: usize = 64 * 1024 * 1024;
pub(crate) const MAX_TLS_ALIGNMENT: usize = 1024 * 1024;

static NEXT_TLS_MODULE_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_TLS_DESCRIPTOR_TOKEN: AtomicU64 = AtomicU64::new(1);
static TLS_DESCRIPTOR_REGISTRY: OnceLock<Mutex<HashMap<u64, Arc<TlsDescriptorContext>>>> =
    OnceLock::new();

thread_local! {
    static GUEST_TLS_BLOCKS: RefCell<HashMap<u64, ThreadTlsAllocation>> =
        RefCell::new(HashMap::new());
}

#[repr(C)]
pub(crate) struct TlsDescriptor {
    pub(crate) resolver: usize,
    pub(crate) token: u64,
}

pub(crate) struct TlsDescriptorContext {
    pub(crate) module: Arc<TlsModule>,
    pub(crate) offset: usize,
    pub(crate) descriptor_address: usize,
    pub(crate) thread_pointer_base: usize,
}

pub(crate) struct TlsModule {
    pub(crate) id: u64,
    pub(crate) memory_size: usize,
    alignment: usize,
    pub(crate) template: Mutex<Vec<u8>>,
    lifecycle: Mutex<TlsLifecycle>,
}

#[derive(Default)]
struct TlsLifecycle {
    accepting: bool,
    active_threads: usize,
}

struct ThreadTlsAllocation {
    module: Arc<TlsModule>,
    pointer: NonNull<u8>,
    layout: Layout,
}

impl ThreadTlsAllocation {
    fn new(module: Arc<TlsModule>) -> Self {
        {
            let mut lifecycle = module
                .lifecycle
                .lock()
                .unwrap_or_else(|_| std::process::abort());
            if !lifecycle.accepting {
                std::process::abort();
            }
            lifecycle.active_threads = lifecycle
                .active_threads
                .checked_add(1)
                .unwrap_or_else(|| std::process::abort());
        }
        let layout = Layout::from_size_align(module.memory_size, module.alignment)
            .unwrap_or_else(|_| std::process::abort());
        // SAFETY: the validated nonzero layout is retained for the matching deallocation.
        let raw = unsafe { alloc_zeroed(layout) };
        let pointer = NonNull::new(raw).unwrap_or_else(|| handle_alloc_error(layout));
        let template = module
            .template
            .lock()
            .unwrap_or_else(|_| std::process::abort());
        // SAFETY: the template is bounded by memory_size and both regions are live/nonoverlapping.
        unsafe { ptr::copy_nonoverlapping(template.as_ptr(), pointer.as_ptr(), template.len()) };
        drop(template);
        Self {
            module,
            pointer,
            layout,
        }
    }
}

impl Drop for ThreadTlsAllocation {
    fn drop(&mut self) {
        // SAFETY: this allocation uniquely owns pointer with the original layout.
        unsafe { dealloc(self.pointer.as_ptr(), self.layout) };
        let mut lifecycle = self
            .module
            .lifecycle
            .lock()
            .unwrap_or_else(|_| std::process::abort());
        lifecycle.active_threads = lifecycle
            .active_threads
            .checked_sub(1)
            .unwrap_or_else(|| std::process::abort());
    }
}

impl TlsModule {
    pub(crate) fn new(header: ProgramHeader, bytes: &[u8]) -> Result<Arc<Self>, LoadError> {
        let memory_size = to_usize(header.memory_size, "PT_TLS memory size")?;
        let alignment = to_usize(header.alignment, "PT_TLS alignment")?;
        let template = checked_slice(
            bytes,
            to_usize(header.offset, "PT_TLS file offset")?,
            to_usize(header.file_size, "PT_TLS file size")?,
            "PT_TLS file range",
        )?
        .to_vec();
        let id = NEXT_TLS_MODULE_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map_err(|_| LoadError::Bounds("TLS module identifier exhausted"))?;
        Ok(Arc::new(Self {
            id,
            memory_size,
            alignment,
            template: Mutex::new(template),
            lifecycle: Mutex::new(TlsLifecycle {
                accepting: true,
                active_threads: 0,
            }),
        }))
    }

    pub(crate) fn seal_for_unload(&self) -> Result<(), LoadError> {
        let mut lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| LoadError::Protection("poisoned TLS lifecycle"))?;
        lifecycle.accepting = false;
        if lifecycle.active_threads != 0 {
            return Err(LoadError::TlsInUse {
                active_threads: lifecycle.active_threads,
            });
        }
        Ok(())
    }
}

#[allow(dead_code)]
pub(crate) fn release_current_thread_tls(module_id: u64) {
    let removed = GUEST_TLS_BLOCKS.try_with(|blocks| {
        blocks
            .try_borrow_mut()
            .unwrap_or_else(|_| std::process::abort())
            .remove(&module_id)
    });
    if removed.is_err() {
        std::process::abort();
    }
}

fn tls_descriptor_registry() -> &'static Mutex<HashMap<u64, Arc<TlsDescriptorContext>>> {
    TLS_DESCRIPTOR_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn register_tls_descriptor(context: TlsDescriptorContext) -> Result<u64, LoadError> {
    let token = NEXT_TLS_DESCRIPTOR_TOKEN
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        })
        .map_err(|_| LoadError::Bounds("TLS descriptor token exhausted"))?;
    let previous = tls_descriptor_registry()
        .lock()
        .map_err(|_| LoadError::Protection("poisoned TLS descriptor registry"))?
        .insert(token, Arc::new(context));
    if previous.is_some() {
        std::process::abort();
    }
    Ok(token)
}

pub(crate) fn unregister_tls_descriptors(tokens: &[u64]) {
    let mut registry = tls_descriptor_registry()
        .lock()
        .unwrap_or_else(|_| std::process::abort());
    for token in tokens {
        if registry.remove(token).is_none() {
            std::process::abort();
        }
    }
}

#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(
    r#"
    .text
    .p2align 2
    .globl _aim_tlsdesc_resolver
_aim_tlsdesc_resolver:
    sub sp, sp, #672
    stp x2, x3, [sp, #0]
    stp x4, x5, [sp, #16]
    stp x6, x7, [sp, #32]
    stp x8, x9, [sp, #48]
    stp x10, x11, [sp, #64]
    stp x12, x13, [sp, #80]
    stp x14, x15, [sp, #96]
    stp x16, x17, [sp, #112]
    stp x18, x30, [sp, #128]
    stp q0, q1, [sp, #144]
    stp q2, q3, [sp, #176]
    stp q4, q5, [sp, #208]
    stp q6, q7, [sp, #240]
    stp q8, q9, [sp, #272]
    stp q10, q11, [sp, #304]
    stp q12, q13, [sp, #336]
    stp q14, q15, [sp, #368]
    stp q16, q17, [sp, #400]
    stp q18, q19, [sp, #432]
    stp q20, q21, [sp, #464]
    stp q22, q23, [sp, #496]
    stp q24, q25, [sp, #528]
    stp q26, q27, [sp, #560]
    stp q28, q29, [sp, #592]
    stp q30, q31, [sp, #624]
    mov x1, x0
    ldr x0, [x0, #8]
    bl _aim_tlsdesc_resolve_impl
    ldp q0, q1, [sp, #144]
    ldp q2, q3, [sp, #176]
    ldp q4, q5, [sp, #208]
    ldp q6, q7, [sp, #240]
    ldp q8, q9, [sp, #272]
    ldp q10, q11, [sp, #304]
    ldp q12, q13, [sp, #336]
    ldp q14, q15, [sp, #368]
    ldp q16, q17, [sp, #400]
    ldp q18, q19, [sp, #432]
    ldp q20, q21, [sp, #464]
    ldp q22, q23, [sp, #496]
    ldp q24, q25, [sp, #528]
    ldp q26, q27, [sp, #560]
    ldp q28, q29, [sp, #592]
    ldp q30, q31, [sp, #624]
    ldp x18, x30, [sp, #128]
    ldp x16, x17, [sp, #112]
    ldp x14, x15, [sp, #96]
    ldp x12, x13, [sp, #80]
    ldp x10, x11, [sp, #64]
    ldp x8, x9, [sp, #48]
    ldp x6, x7, [sp, #32]
    ldp x4, x5, [sp, #16]
    ldp x2, x3, [sp, #0]
    add sp, sp, #672
    ret
"#
);

#[cfg(target_arch = "aarch64")]
unsafe extern "C" {
    pub(crate) fn aim_tlsdesc_resolver(descriptor: *const TlsDescriptor) -> isize;
}

#[cfg(target_arch = "aarch64")]
#[unsafe(no_mangle)]
unsafe extern "C" fn aim_tlsdesc_resolve_impl(token: u64, descriptor_address: usize) -> isize {
    let context = tls_descriptor_registry()
        .lock()
        .unwrap_or_else(|_| std::process::abort())
        .get(&token)
        .cloned()
        .unwrap_or_else(|| std::process::abort());
    if context.descriptor_address != descriptor_address {
        std::process::abort();
    }
    let storage = GUEST_TLS_BLOCKS
        .try_with(|blocks| {
            let mut blocks = blocks
                .try_borrow_mut()
                .unwrap_or_else(|_| std::process::abort());
            let allocation = blocks
                .entry(context.module.id)
                .or_insert_with(|| ThreadTlsAllocation::new(context.module.clone()));
            // SAFETY: the relocation validated offset inside this allocation.
            unsafe { allocation.pointer.as_ptr().add(context.offset) as usize }
        })
        .unwrap_or_else(|_| std::process::abort());
    // Guest MRS instructions are rewritten to this image-local compatibility
    // base. Returning an offset from the same base preserves the AArch64
    // TLSDESC contract while leaving Darwin's TPIDR_EL0 ownership untouched.
    storage.wrapping_sub(context.thread_pointer_base) as isize
}
