use super::{Capability, LoadError};

const LINUX_SVC_ZERO: u32 = 0xd400_0001;
const B: u32 = 0x1400_0000;
const B_MIN_BYTES: i128 = -(1_i128 << 27);
const B_MAX_BYTES: i128 = (1_i128 << 27) - 4;

// Fixed-size slots make the direct-SVC page reservation deterministic while
// allowing each rewritten site to use a normal B (which preserves x30).
pub(crate) const DIRECT_SYSCALL_VENEER_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DirectSyscallRewrite {
    pub instruction_index: usize,
    pub original: u32,
    pub veneer_offset: usize,
    pub return_address: usize,
}

pub(crate) fn contains_linux_svc(code: &[u8]) -> bool {
    code.chunks_exact(4)
        .any(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()) == LINUX_SVC_ZERO)
}

pub(crate) fn count_linux_svc(code: &[u8]) -> usize {
    code.chunks_exact(4)
        .filter(|bytes| u32::from_le_bytes((*bytes).try_into().unwrap()) == LINUX_SVC_ZERO)
        .count()
}

pub(crate) fn veneer_page_count(svc_count: usize, page_size: usize) -> Result<usize, LoadError> {
    if page_size == 0 || !page_size.is_power_of_two() {
        return Err(LoadError::Bounds("direct-syscall page size"));
    }
    let bytes = svc_count
        .checked_mul(DIRECT_SYSCALL_VENEER_BYTES)
        .ok_or(LoadError::Bounds("direct-syscall veneer size overflow"))?;
    Ok(bytes
        .checked_add(page_size - 1)
        .ok_or(LoadError::Bounds("direct-syscall veneer pages overflow"))?
        / page_size)
}

pub(crate) fn rewrite_linux_svc(
    code: &mut [u8],
    veneer_base: usize,
    veneer_cursor: &mut usize,
    veneer_capacity: usize,
) -> Result<Vec<DirectSyscallRewrite>, LoadError> {
    let mut rewrites = Vec::new();
    for instruction_index in 0..code.len() / 4 {
        if read_instruction(code, instruction_index) != LINUX_SVC_ZERO {
            continue;
        }
        let instruction_address = code.as_ptr() as usize + instruction_index * 4;
        let veneer_offset = *veneer_cursor;
        let veneer_address = veneer_base
            .checked_add(veneer_offset)
            .ok_or(LoadError::Bounds("direct-syscall veneer address overflow"))?;
        let return_address = instruction_address
            .checked_add(4)
            .ok_or(LoadError::Bounds("direct-syscall return address overflow"))?;
        check_branch(instruction_address, veneer_address)?;
        check_branch(veneer_address, return_address)?;
        if veneer_offset
            .checked_add(DIRECT_SYSCALL_VENEER_BYTES)
            .is_none_or(|end| end > veneer_capacity)
        {
            return Err(LoadError::Bounds("direct-syscall veneer page capacity"));
        }
        let immediate = branch_immediate(instruction_address, veneer_address)?;
        write_instruction(code, instruction_index, B | immediate);
        rewrites.push(DirectSyscallRewrite {
            instruction_index,
            original: LINUX_SVC_ZERO,
            veneer_offset,
            return_address,
        });
        *veneer_cursor = veneer_offset
            .checked_add(DIRECT_SYSCALL_VENEER_BYTES)
            .ok_or(LoadError::Bounds("direct-syscall veneer cursor overflow"))?;
    }
    Ok(rewrites)
}

/// Writes one per-site veneer. It preserves the guest state across the
/// Bionic C call, converts Bionic's -1/errno result to Linux raw -errno, and
/// branches to the instruction after the rewritten SVC.
///
/// # Safety
///
/// `destination` must reference at least `DIRECT_SYSCALL_VENEER_BYTES`
/// writable, instruction-aligned bytes in a loader-owned page. The two target
/// addresses must be valid AArch64 function addresses.
pub(crate) unsafe fn write_veneer(
    destination: *mut u8,
    syscall_target: usize,
    errno_target: usize,
    return_address: usize,
) -> Result<(), LoadError> {
    if destination.is_null()
        || destination as usize & 3 != 0
        || syscall_target == 0
        || errno_target == 0
    {
        return Err(LoadError::Bounds("direct-syscall veneer address"));
    }
    let mut instructions = Vec::with_capacity(DIRECT_SYSCALL_VENEER_BYTES / 4);
    instructions.push(0xd110_03ff); // sub sp, sp, #0x400
    for register in (0..=28).step_by(2) {
        instructions.push(stp_x(register, register + 1, (register / 2 * 16) as usize));
    }
    instructions.push(str_x(30, 240));
    instructions.extend([
        0xd53b_4209, // mrs x9, nzcv
        str_x(9, 248),
        0xd53b_4429, // mrs x9, fpsr
        str_x(9, 256),
        0xd53b_4409, // mrs x9, fpcr
        str_x(9, 264),
    ]);
    for register in (0..=30).step_by(2) {
        instructions.push(stp_q(
            register,
            register + 1,
            (272 + register / 2 * 32) as usize,
        ));
    }

    // Preserve the caller's errno before the Bionic shim can overwrite it.
    instructions.extend(load_immediate_x16(errno_target as u64));
    instructions.push(0xd63f_0200); // blr x16
    instructions.push(0xb940_0009); // ldr w9, [x0]
    instructions.push(str_w(9, 784));
    instructions.push(str_x(0, 792)); // stable errno pointer for restore

    // Restore x0..x8, then shift Linux's x0..x5 arguments for varargs syscall.
    for register in 0..=8 {
        instructions.push(ldr_x(register, (register * 8) as usize));
    }
    instructions.extend([
        mov_x(6, 5),
        mov_x(5, 4),
        mov_x(4, 3),
        mov_x(3, 2),
        mov_x(2, 1),
        mov_x(1, 0),
        mov_x(0, 8),
    ]);
    instructions.extend(load_immediate_x16(syscall_target as u64));
    instructions.push(0xd63f_0200); // blr x16
    instructions.push(0xb100_041f); // cmn x0, #1
    let branch_instruction = instructions.len();
    instructions.push(0); // b.ne errno restore
    instructions.push(ldr_x(10, 792));
    instructions.push(ldr_w_reg(9, 10));
    instructions.push(0xcb09_03e0); // neg x0, x9
    let errno_restore = instructions.len();
    instructions.push(ldr_x(10, 792));
    instructions.push(ldr_w(11, 784));
    instructions.push(str_w_reg(11, 10));
    let branch_delta = (errno_restore as i128 - branch_instruction as i128) * 4;
    instructions[branch_instruction] = 0x5400_0001 | cond_branch_imm(branch_delta)?;

    for register in (0..=30).step_by(2) {
        instructions.push(ldp_q(
            register,
            register + 1,
            (272 + register / 2 * 32) as usize,
        ));
    }
    instructions.extend([
        ldr_x(9, 248),
        0xd51b_4209, // msr nzcv, x9
        ldr_x(9, 256),
        0xd51b_4429, // msr fpsr, x9
        ldr_x(9, 264),
        0xd51b_4409, // msr fpcr, x9
    ]);
    for register in (1..=28).step_by(2) {
        instructions.push(ldp_x(
            register,
            register + 1,
            (register / 2 * 16 + 8) as usize,
        ));
    }
    instructions.push(ldr_x(29, 232));
    instructions.push(ldr_x(30, 240));
    instructions.push(0x9110_03ff); // add sp, sp, #0x400
    let branch_from = destination as usize + instructions.len() * 4;
    instructions.push(B | branch_immediate(branch_from, return_address)?);
    if instructions.len() * 4 > DIRECT_SYSCALL_VENEER_BYTES {
        return Err(LoadError::Bounds(
            "direct-syscall veneer instruction budget",
        ));
    }
    for (index, instruction) in instructions.into_iter().enumerate() {
        // SAFETY: guaranteed by the caller; unaligned writes avoid Rust refs.
        unsafe {
            destination
                .add(index * 4)
                .cast::<u32>()
                .write_unaligned(instruction)
        };
    }
    Ok(())
}

fn check_branch(from: usize, to: usize) -> Result<(), LoadError> {
    let delta = to as i128 - from as i128;
    if delta & 3 != 0 || !(B_MIN_BYTES..=B_MAX_BYTES).contains(&delta) {
        return Err(LoadError::Capability(Capability::DirectSyscall));
    }
    Ok(())
}

fn branch_immediate(from: usize, to: usize) -> Result<u32, LoadError> {
    check_branch(from, to)?;
    Ok((((to as i128 - from as i128) >> 2) as i64 as u64 as u32) & 0x03ff_ffff)
}

fn cond_branch_imm(delta: i128) -> Result<u32, LoadError> {
    if delta & 3 != 0 || !(-(1_i128 << 20)..=((1_i128 << 20) - 4)).contains(&delta) {
        return Err(LoadError::Bounds("direct-syscall conditional branch"));
    }
    Ok((((delta >> 2) as i64 as u64 as u32) & 0x7ffff) << 5)
}

fn mov_x(destination: u32, source: u32) -> u32 {
    0xaa00_03e0 | (source << 16) | destination
}

fn load_immediate_x16(value: u64) -> [u32; 4] {
    [
        0xd280_0010 | (((value & 0xffff) as u32) << 5),
        0xf2a0_0010 | ((((value >> 16) & 0xffff) as u32) << 5),
        0xf2c0_0010 | ((((value >> 32) & 0xffff) as u32) << 5),
        0xf2e0_0010 | ((((value >> 48) & 0xffff) as u32) << 5),
    ]
}

fn stp_x(first: u32, second: u32, offset: usize) -> u32 {
    0xa900_0000 | ((offset as u32 / 8) << 15) | (second << 10) | (31 << 5) | first
}

fn ldp_x(first: u32, second: u32, offset: usize) -> u32 {
    0xa940_0000 | ((offset as u32 / 8) << 15) | (second << 10) | (31 << 5) | first
}

fn str_x(register: u32, offset: usize) -> u32 {
    0xf900_0000 | ((offset as u32 / 8) << 10) | (31 << 5) | register
}

fn ldr_x(register: u32, offset: usize) -> u32 {
    0xf940_0000 | ((offset as u32 / 8) << 10) | (31 << 5) | register
}

fn str_w(register: u32, offset: usize) -> u32 {
    0xb900_0000 | ((offset as u32 / 4) << 10) | (31 << 5) | register
}

fn ldr_w(register: u32, offset: usize) -> u32 {
    0xb940_0000 | ((offset as u32 / 4) << 10) | (31 << 5) | register
}

fn ldr_w_reg(destination: u32, base: u32) -> u32 {
    0xb940_0000 | (base << 5) | destination
}

fn str_w_reg(source: u32, base: u32) -> u32 {
    0xb900_0000 | (base << 5) | source
}

fn stp_q(first: u32, second: u32, offset: usize) -> u32 {
    0xad00_0000 | ((offset as u32 / 16) << 15) | (second << 10) | (31 << 5) | first
}

fn ldp_q(first: u32, second: u32, offset: usize) -> u32 {
    0xad40_0000 | ((offset as u32 / 16) << 15) | (second << 10) | (31 << 5) | first
}

fn read_instruction(code: &[u8], index: usize) -> u32 {
    let offset = index * 4;
    u32::from_le_bytes(code[offset..offset + 4].try_into().unwrap())
}

fn write_instruction(code: &mut [u8], index: usize, instruction: u32) {
    let offset = index * 4;
    code[offset..offset + 4].copy_from_slice(&instruction.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_each_linux_svc_to_a_distinct_near_b() {
        let mut code = [LINUX_SVC_ZERO, 0xd503_201f, LINUX_SVC_ZERO]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>();
        let veneer = code.as_ptr() as usize + 0x100;
        let mut cursor = 0;
        let rewrites = rewrite_linux_svc(&mut code, veneer, &mut cursor, 1024).unwrap();
        assert_eq!(rewrites.len(), 2);
        assert_ne!(rewrites[0].veneer_offset, rewrites[1].veneer_offset);
        assert_eq!(read_instruction(&code, 0) & 0xfc00_0000, B);
        assert_eq!(read_instruction(&code, 2) & 0xfc00_0000, B);
        assert_eq!(rewrites[0].return_address, code.as_ptr() as usize + 4);
    }

    #[test]
    fn rejects_a_veneer_outside_branch_range() {
        let mut code = LINUX_SVC_ZERO.to_le_bytes().to_vec();
        let far = (code.as_ptr() as usize).wrapping_add(1 << 28);
        let mut cursor = 0;
        assert!(matches!(
            rewrite_linux_svc(&mut code, far, &mut cursor, 1024),
            Err(LoadError::Capability(Capability::DirectSyscall))
        ));
    }

    #[test]
    fn veneer_has_register_state_and_raw_result_conversion() {
        let mut storage = [0_u8; DIRECT_SYSCALL_VENEER_BYTES];
        unsafe {
            write_veneer(
                storage.as_mut_ptr(),
                0x1234_5678_9abc_def0,
                0x2345_6789_abcd_ef01,
                storage.as_ptr() as usize + 0x800,
            )
            .unwrap()
        };
        let words = storage
            .chunks_exact(4)
            .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
            .collect::<Vec<_>>();
        assert_eq!(words[0], 0xd110_03ff);
        assert!(words.contains(&0xcb09_03e0)); // neg errno
        assert!(words.contains(&0xd53b_4209)); // save NZCV
        assert!(words.iter().any(|word| word & 0xffc0_03ff == 0xad00_03e0)); // save SIMD pair
        let add = words.iter().position(|word| *word == 0x9110_03ff).unwrap();
        assert_eq!(words[add + 1] & 0xfc00_0000, B);
    }

    // Executes the generated veneer on arm64 and catches the former BL/LR
    // loop: the post-SVC RET must return to the Rust caller.
    #[cfg(target_arch = "aarch64")]
    #[test]
    fn executable_veneer_returns_raw_errno_after_post_svc() {
        use std::sync::atomic::{AtomicBool, Ordering};

        static FAIL: AtomicBool = AtomicBool::new(true);
        static mut ERRNO: i32 = 7;

        unsafe extern "C" fn syscall_mock(
            number: u64,
            _: u64,
            _: u64,
            _: u64,
            _: u64,
            _: u64,
            _: u64,
        ) -> i64 {
            if number != 172 {
                return -38;
            }
            if FAIL.load(Ordering::Relaxed) {
                unsafe { ERRNO = 38 };
                -1
            } else {
                unsafe { ERRNO = 99 };
                123
            }
        }
        unsafe extern "C" fn errno_mock() -> *mut i32 {
            std::ptr::addr_of_mut!(ERRNO)
        }
        unsafe {
            let page = super::super::mmap(
                std::ptr::null_mut(),
                16 * 1024,
                super::super::PROT_READ | super::super::PROT_WRITE,
                super::super::MAP_PRIVATE | super::super::MAP_ANON,
                -1,
                0,
            );
            assert!(!page.is_null() && page as isize != -1);
            let code = page.cast::<u8>();
            code.cast::<u32>().write_unaligned(0xd280_1588); // mov x8,#172
            code.add(4).cast::<u32>().write_unaligned(
                B | branch_immediate(code as usize + 4, code as usize + 0x100).unwrap(),
            );
            code.add(8).cast::<u32>().write_unaligned(0xd65f_03c0); // ret
            write_veneer(
                code.add(0x100),
                syscall_mock as usize,
                errno_mock as usize,
                code as usize + 8,
            )
            .unwrap();
            super::super::sys_icache_invalidate(page, 16 * 1024);
            assert_eq!(
                super::super::mprotect(
                    page,
                    16 * 1024,
                    super::super::PROT_READ | super::super::PROT_EXEC
                ),
                0
            );
            let entry: unsafe extern "C" fn(u64) -> i64 = std::mem::transmute(code);
            assert_eq!(entry(0), -38);
            assert_eq!(std::ptr::read_volatile(std::ptr::addr_of!(ERRNO)), 7);
            FAIL.store(false, Ordering::Relaxed);
            assert_eq!(entry(0), 123);
            assert_eq!(std::ptr::read_volatile(std::ptr::addr_of!(ERRNO)), 7);
            assert_eq!(super::super::munmap(page, 16 * 1024), 0);
        }
    }
}
