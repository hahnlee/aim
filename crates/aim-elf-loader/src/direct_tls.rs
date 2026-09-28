use super::{Capability, LoadError};

const MRS_TPIDR_EL0: u32 = 0xd53b_d040;
const MRS_REGISTER_MASK: u32 = 0xffff_ffe0;
const ADRP: u32 = 0x9000_0000;
const LDR_X_UNSIGNED_IMMEDIATE: u32 = 0xf940_0000;
const LDR_X_UNSIGNED_IMMEDIATE_MASK: u32 = 0xffc0_0000;
const BLR: u32 = 0xd63f_0000;
const BLR_REGISTER_MASK: u32 = 0xffff_fc1f;
#[cfg(test)]
const ADD_X_SHIFTED_REGISTER: u32 = 0x8b00_0000;
const ADD_X_IMMEDIATE: u32 = 0x9100_0000;
const ADD_X_IMMEDIATE_MASK: u32 = 0xffc0_0000;
const ADRP_X0: u32 = 0x9000_0000;
const ADRP_REGISTER_MASK: u32 = 0x9f00_001f;
#[cfg(test)]
const MOV_X_REGISTER: u32 = 0xaa00_03e0;
const ANDROID_TLS_SLOT_STACK_GUARD_OFFSET: u32 = 0x28;
// LLVM normally emits the guard load immediately after reading TPIDR_EL0, but
// highly optimized third-party code can schedule independent arithmetic between
// the two. Real optimized DSOs can have dozens of intervening instructions.
// Keep this bounded to a small basic-block-sized window: the second half of the
// pair must still be an exact `LDR Xt, [Xthread, #0x28]` using the same register.
const MAX_STACK_GUARD_DISTANCE_IN_INSTRUCTIONS: usize = 64;
// Optimized Chromium code schedules flag calculations and unrelated loads between
// the TLSDESC resolver call and the eventual TPIDR_EL0 read. Keep the same bounded
// basic-block-sized window used for stack guards while still requiring the exact
// ADRP/LDR/ADD/BLR descriptor-call shape.
const MAX_TLSDESC_RESULT_DISTANCE_IN_INSTRUCTIONS: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct StackGuardRewrite {
    pub instruction_index: usize,
    pub original: u32,
}

/// Rewrites Bionic's direct stack-guard TLS sequence into a Darwin-safe equivalent.
///
/// Android arm64 code commonly reads `TLS_SLOT_STACK_GUARD` with an `MRS TPIDR_EL0`
/// followed shortly by `LDR Xt, [Xthread, #0x28]`. Darwin owns TPIDR_EL0 and neither
/// TPIDR_EL0 nor TPIDRRO_EL0 provides Android's immutable stack-guard slot. In particular,
/// Darwin may mutate the value at `TPIDRRO_EL0 + 0x28` while a guest function is active.
/// The rewrite redirects the validated Android stack-guard base to a loader-owned,
/// read-only page whose guard remains at the ABI's original `+0x28` offset:
///
/// `ADRP Xthread, loader_guard_page; ...; LDR Xt, [Xthread, #0x28]`
///
/// The load stays intact because one compiler-generated function can reuse the single
/// thread-base read for both its prologue and every epilogue guard comparison.
///
/// Local TLSDESC accesses and the stack guard may reuse the same thread-base read.
/// The loader therefore redirects every validated read to the compatibility page;
/// `tls.rs` returns `guest_address - compatibility_page`, preserving the compiler's
/// `BLR resolver; MRS thread; ADD/load/store [thread, offset]` result without touching
/// Darwin's thread pointer.
///
/// This is deliberately not a generic Android TLS emulation. Any direct TPIDR_EL0 use
/// that is neither the validated stack-guard pattern nor the exact local-TLSDESC sequence
/// fails loading instead of executing with subtly incorrect TLS semantics.
pub(crate) fn rewrite_android_stack_guard_tls(
    code: &mut [u8],
    stack_guard_address: usize,
) -> Result<Vec<StackGuardRewrite>, LoadError> {
    if stack_guard_address & 0xfff != ANDROID_TLS_SLOT_STACK_GUARD_OFFSET as usize {
        return Err(LoadError::Bounds("stack-guard page offset"));
    }
    let instruction_count = code.len() / 4;
    let mut rewrites = Vec::new();
    let mut unclassified = 0usize;

    for instruction_index in 0..instruction_count {
        let instruction = read_instruction(code, instruction_index);
        if instruction & MRS_REGISTER_MASK != MRS_TPIDR_EL0 {
            continue;
        }

        let thread_register = instruction & 0x1f;
        let search_end = instruction_index
            .saturating_add(MAX_STACK_GUARD_DISTANCE_IN_INSTRUCTIONS + 1)
            .min(instruction_count);
        let guard_load = (instruction_index + 1..search_end).find(|candidate_index| {
            is_stack_guard_load(read_instruction(code, *candidate_index), thread_register)
        });
        let Some(guard_load) = guard_load else {
            if is_local_tlsdesc_result(code, instruction_index, thread_register) {
                rewrites.push((instruction_index, instruction_index));
                continue;
            }
            if unclassified < 32 {
                let context_start = instruction_index.saturating_sub(8);
                let context_end = (instruction_index + 9).min(instruction_count);
                let context = (context_start..context_end)
                    .map(|index| format!("{index:#x}:{:#010x}", read_instruction(code, index)))
                    .collect::<Vec<_>>()
                    .join(" ");
                eprintln!(
                    "DARWIN ELF TLS: unclassified TPIDR_EL0 instruction={instruction_index:#x} register=x{thread_register} context=[{context}]"
                );
            }
            unclassified += 1;
            continue;
        };
        rewrites.push((instruction_index, guard_load));
    }

    if unclassified != 0 {
        eprintln!("DARWIN ELF TLS: unclassified TPIDR_EL0 total={unclassified}");
        return Err(LoadError::Capability(Capability::DirectThreadPointer));
    }

    let mut applied = Vec::with_capacity(rewrites.len());
    for (thread_pointer_index, _) in &rewrites {
        let original_mrs = read_instruction(code, *thread_pointer_index);
        let thread_register = original_mrs & 0x1f;
        let instruction_address = code.as_ptr() as usize + *thread_pointer_index * 4;
        let instruction_page = instruction_address & !0xfff;
        let guard_page = stack_guard_address & !0xfff;
        let page_delta = (guard_page as i128 - instruction_page as i128) >> 12;
        if !(-(1_i128 << 20)..(1_i128 << 20)).contains(&page_delta) {
            return Err(LoadError::Bounds("stack-guard ADRP range"));
        }
        let immediate = (page_delta as i64 as u64) & 0x1f_ffff;
        let adrp = ADRP
            | (((immediate & 0x3) as u32) << 29)
            | ((((immediate >> 2) & 0x7_ffff) as u32) << 5)
            | thread_register;
        write_instruction(code, *thread_pointer_index, adrp);
        applied.push(StackGuardRewrite {
            instruction_index: *thread_pointer_index,
            original: original_mrs,
        });
    }

    Ok(applied)
}

fn is_local_tlsdesc_result(code: &[u8], instruction_index: usize, _thread_register: u32) -> bool {
    let instruction_count = code.len() / 4;
    if instruction_index + 1 >= instruction_count {
        return false;
    }

    if is_preread_local_tlsdesc_result(code, instruction_index) {
        return true;
    }
    if instruction_index < 4 {
        return false;
    }

    let search_start = instruction_index
        .saturating_sub(MAX_TLSDESC_RESULT_DISTANCE_IN_INSTRUCTIONS)
        .max(3);
    (search_start..instruction_index)
        .rev()
        .any(|candidate| is_local_tlsdesc_call(code, candidate))
}

fn is_preread_local_tlsdesc_result(code: &[u8], instruction_index: usize) -> bool {
    let instruction_count = code.len() / 4;
    let call_search_end = instruction_index
        .saturating_add(MAX_TLSDESC_RESULT_DISTANCE_IN_INSTRUCTIONS + 1)
        .min(instruction_count);
    (instruction_index + 1..call_search_end).any(|candidate| is_local_tlsdesc_call(code, candidate))
}

fn is_local_tlsdesc_call(code: &[u8], call_index: usize) -> bool {
    if call_index < 3 || read_instruction(code, call_index) & BLR_REGISTER_MASK != BLR {
        return false;
    }
    let page = read_instruction(code, call_index - 3);
    let descriptor_load = read_instruction(code, call_index - 2);
    let descriptor_address = read_instruction(code, call_index - 1);
    page & ADRP_REGISTER_MASK == ADRP_X0
        && descriptor_load & LDR_X_UNSIGNED_IMMEDIATE_MASK == LDR_X_UNSIGNED_IMMEDIATE
        && descriptor_load & 0x1f == 1
        && (descriptor_load >> 5) & 0x1f == 0
        && descriptor_address & ADD_X_IMMEDIATE_MASK == ADD_X_IMMEDIATE
        && descriptor_address & 0x1f == 0
        && (descriptor_address >> 5) & 0x1f == 0
}

fn is_stack_guard_load(instruction: u32, thread_register: u32) -> bool {
    instruction & LDR_X_UNSIGNED_IMMEDIATE_MASK == LDR_X_UNSIGNED_IMMEDIATE
        && (instruction >> 5) & 0x1f == thread_register
        && ((instruction >> 10) & 0xfff) * 8 == ANDROID_TLS_SLOT_STACK_GUARD_OFFSET
}

fn read_instruction(code: &[u8], index: usize) -> u32 {
    let offset = index * 4;
    u32::from_le_bytes(
        code[offset..offset + 4]
            .try_into()
            .expect("bounded instruction"),
    )
}

fn write_instruction(code: &mut [u8], index: usize, instruction: u32) {
    let offset = index * 4;
    code[offset..offset + 4].copy_from_slice(&instruction.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instructions(values: &[u32]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|instruction| instruction.to_le_bytes())
            .collect()
    }

    // A Vec<u8> may begin close enough to a page boundary that the tested MRS
    // instruction lives on the following page. Derive the synthetic guard
    // page from the instruction under test rather than the allocation base;
    // otherwise the expected ADRP immediate depends on allocator placement
    // and the test becomes intermittently flaky under parallel execution.
    fn guard_for(code: &[u8], instruction_index: usize) -> usize {
        (code.as_ptr() as usize + instruction_index * 4) & !0xfff | 0x28
    }

    #[test]
    fn rewrites_only_the_validated_android_stack_guard_pair() {
        let mut code = instructions(&[
            MRS_TPIDR_EL0 | 19,
            0xaa00_03f4,
            LDR_X_UNSIGNED_IMMEDIATE | (5 << 10) | (19 << 5) | 8,
        ]);

        let guard = guard_for(&code, 0);
        assert_eq!(
            rewrite_android_stack_guard_tls(&mut code, guard)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(read_instruction(&code, 0), ADRP | 19);
        assert_eq!(read_instruction(&code, 1), 0xaa00_03f4);
        assert_eq!(
            read_instruction(&code, 2),
            LDR_X_UNSIGNED_IMMEDIATE | (5 << 10) | (19 << 5) | 8
        );
    }

    #[test]
    fn rejects_a_general_direct_android_tls_access() {
        let mut code = instructions(&[
            MRS_TPIDR_EL0 | 20,
            LDR_X_UNSIGNED_IMMEDIATE | (7 << 10) | (20 << 5),
        ]);
        let guard = guard_for(&code, 0);

        assert!(matches!(
            rewrite_android_stack_guard_tls(&mut code, guard),
            Err(LoadError::Capability(Capability::DirectThreadPointer))
        ));
    }

    #[test]
    fn accepts_a_scheduled_stack_guard_load() {
        let mut values = vec![MRS_TPIDR_EL0 | 19];
        values.extend(std::iter::repeat_n(0xd503_201f, 33));
        values.push(LDR_X_UNSIGNED_IMMEDIATE | (5 << 10) | (19 << 5) | 8);
        let mut code = instructions(&values);
        let guard = guard_for(&code, 0);

        assert_eq!(
            rewrite_android_stack_guard_tls(&mut code, guard)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(read_instruction(&code, 0), ADRP | 19);
    }

    #[test]
    fn rewrites_the_exact_local_tlsdesc_result_sequence() {
        let mut code = instructions(&[
            ADRP_X0,
            LDR_X_UNSIGNED_IMMEDIATE | 1,
            ADD_X_IMMEDIATE,
            BLR | (1 << 5),
            MRS_TPIDR_EL0 | 8,
            ADD_X_SHIFTED_REGISTER | (8 << 5),
            0xd65f_03c0,
        ]);

        let guard = guard_for(&code, 4);
        assert_eq!(
            rewrite_android_stack_guard_tls(&mut code, guard)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(read_instruction(&code, 4), ADRP | 8);
    }

    #[test]
    fn rewrites_a_preread_local_tlsdesc_result_sequence() {
        let mut code = instructions(&[
            MRS_TPIDR_EL0 | 21,
            ADRP_X0,
            LDR_X_UNSIGNED_IMMEDIATE | 1,
            ADD_X_IMMEDIATE,
            BLR | (1 << 5),
            ADD_X_SHIFTED_REGISTER | (21 << 5) | 20,
            0xd65f_03c0,
        ]);

        let guard = guard_for(&code, 0);
        assert_eq!(
            rewrite_android_stack_guard_tls(&mut code, guard)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(read_instruction(&code, 0), ADRP | 21);
    }

    #[test]
    fn accepts_a_chromium_scheduled_tlsdesc_indexed_load() {
        let mut values = vec![
            ADRP_X0,
            LDR_X_UNSIGNED_IMMEDIATE | 1,
            ADD_X_IMMEDIATE,
            BLR | (1 << 5),
        ];
        values.extend(std::iter::repeat_n(0xd503_201f, 19));
        let thread_pointer_index = values.len();
        values.push(MRS_TPIDR_EL0 | 8);
        values.push(0xf860_6800 | (8 << 5));
        let mut code = instructions(&values);

        let guard = guard_for(&code, thread_pointer_index);
        assert_eq!(
            rewrite_android_stack_guard_tls(&mut code, guard)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(read_instruction(&code, thread_pointer_index), ADRP | 8);
    }

    #[test]
    fn accepts_a_chromium_delayed_tlsdesc_indexed_load() {
        let mut values = vec![
            ADRP_X0,
            LDR_X_UNSIGNED_IMMEDIATE | 1,
            ADD_X_IMMEDIATE,
            BLR | (1 << 5),
        ];
        values.extend(std::iter::repeat_n(0xd503_201f, 13));
        let thread_pointer_index = values.len();
        values.push(MRS_TPIDR_EL0 | 13);
        values.extend(std::iter::repeat_n(0xd503_201f, 34));
        values.push(0xf860_6800 | (13 << 5));
        let mut code = instructions(&values);

        let guard = guard_for(&code, thread_pointer_index);
        assert_eq!(
            rewrite_android_stack_guard_tls(&mut code, guard)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(read_instruction(&code, thread_pointer_index), ADRP | 13);
    }

    #[test]
    fn accepts_a_chromium_spilled_tlsdesc_pair() {
        let mut values = vec![
            ADRP_X0,
            LDR_X_UNSIGNED_IMMEDIATE | 1,
            ADD_X_IMMEDIATE,
            BLR | (1 << 5),
            MRS_TPIDR_EL0 | 8,
            0xa901_83e8, // stp x8, x0, [sp, #0x18]
        ];
        values.extend(std::iter::repeat_n(0xd503_201f, 104));
        values.push(0xa941_a3eb); // ldp x11, x8, [sp, #0x18]
        values.push(0xf868_6968); // ldr x8, [x11, x8]
        let mut code = instructions(&values);

        let guard = guard_for(&code, 4);
        assert_eq!(
            rewrite_android_stack_guard_tls(&mut code, guard)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(read_instruction(&code, 4), ADRP | 8);
    }

    #[test]
    fn rejects_tlsdesc_like_arithmetic_without_a_resolver_call() {
        let mut code = instructions(&[
            0xd503_201f,
            MRS_TPIDR_EL0 | 8,
            ADD_X_SHIFTED_REGISTER | (8 << 5),
        ]);
        let guard = guard_for(&code, 1);

        assert!(matches!(
            rewrite_android_stack_guard_tls(&mut code, guard),
            Err(LoadError::Capability(Capability::DirectThreadPointer))
        ));
    }

    #[test]
    fn accepts_optimized_tlsdesc_indexed_load_and_moved_offset() {
        let mut code = instructions(&[
            ADRP_X0,
            LDR_X_UNSIGNED_IMMEDIATE | 1,
            ADD_X_IMMEDIATE,
            BLR | (1 << 5),
            MOV_X_REGISTER | 8,
            0x9b0a_2440,
            MRS_TPIDR_EL0 | 9,
            ADD_X_SHIFTED_REGISTER | (8 << 16) | (9 << 5) | 8,
            ADRP_X0,
            LDR_X_UNSIGNED_IMMEDIATE | 1,
            ADD_X_IMMEDIATE,
            BLR | (1 << 5),
            MRS_TPIDR_EL0 | 20,
            0xf860_6800 | (20 << 5),
        ]);

        let guard = guard_for(&code, 6);
        assert_eq!(
            rewrite_android_stack_guard_tls(&mut code, guard)
                .unwrap()
                .len(),
            2
        );
        // These sites can straddle a 4 KiB page even in this small allocation.
        // Verify the decoded target, not an assumed zero ADRP page displacement.
        for (index, register) in [(6, 9), (12, 20)] {
            let instruction = read_instruction(&code, index);
            assert_eq!(instruction & ADRP_REGISTER_MASK, ADRP | register);
            let immediate = ((instruction >> 29) & 3) | (((instruction >> 5) & 0x7ffff) << 2);
            let pages = ((immediate as i64) << 43) >> 43;
            let pc_page = (code.as_ptr() as usize + index * 4) & !0xfff;
            assert_eq!(
                pc_page.wrapping_add_signed((pages << 12) as isize),
                guard & !0xfff
            );
        }
    }

    #[test]
    fn leaves_code_without_direct_android_tls_untouched() {
        let original = instructions(&[0xd503_201f, 0xd65f_03c0]);
        let mut code = original.clone();

        let guard = guard_for(&code, 0);
        assert!(
            rewrite_android_stack_guard_tls(&mut code, guard)
                .unwrap()
                .is_empty()
        );
        assert_eq!(code, original);
    }
}
