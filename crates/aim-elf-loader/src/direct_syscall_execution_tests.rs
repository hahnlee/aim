//! Execution-level coverage for the generated per-site direct-SVC veneer.
//!
//! The test deliberately runs the veneer in an executable mapping and makes
//! both call targets destroy caller-clobbered GPRs, SIMD registers, and NZCV.
//! It is kept in a separate module so the implementation file remains the
//! single owner of the instruction encoder.

#[cfg(all(target_arch = "aarch64", target_os = "macos"))]
mod arm64 {
    use super::super::direct_syscall;
    use super::super::{
        MAP_ANON, MAP_PRIVATE, PROT_EXEC, PROT_READ, PROT_WRITE, mmap, mprotect, munmap,
        sys_icache_invalidate,
    };
    use std::ptr;

    const PAGE_SIZE: usize = 16 * 1024;
    const VENEER_OFFSET: usize = 0x1000;
    const ENTRY_STACK_POINTER: usize = 0;
    const ENTRY_SAVED_X30: usize = 8;
    const ENTRY_RESULT: usize = 16;
    const ENTRY_REGS: usize = 32;
    const ENTRY_Q0: usize = 288;
    const ENTRY_Q16: usize = 304;
    const ENTRY_NZCV: usize = 320;

    #[repr(C)]
    struct Snapshot {
        result: i64,
        expected_x30: u64,
        registers: [u64; 30],
        q0: [u8; 16],
        q16: [u8; 16],
        nzcv: u64,
    }

    #[unsafe(no_mangle)]
    pub static mut AIM_DIRECT_TEST_ERRNO: i32 = 38;

    core::arch::global_asm!(
        r#"
        .text
        .p2align 2
        .globl _aim_direct_test_invoke
_aim_direct_test_invoke:
        sub sp, sp, #112
        stp x19, x20, [sp]
        stp x21, x22, [sp, #16]
        stp x23, x24, [sp, #32]
        stp x25, x26, [sp, #48]
        stp x27, x28, [sp, #64]
        stp x29, x30, [sp, #80]
        str x18, [sp, #96]
        mov x9, #0x6000
        lsl x9, x9, #16
        msr nzcv, x9
        mov x9, x0
        mov x0, x1
        movi v0.16b, #0x11
        movi v16.16b, #0x22
        blr x9
        ldp x19, x20, [sp]
        ldp x21, x22, [sp, #16]
        ldp x23, x24, [sp, #32]
        ldp x25, x26, [sp, #48]
        ldp x27, x28, [sp, #64]
        ldp x29, x30, [sp, #80]
        ldr x18, [sp, #96]
        add sp, sp, #112
        ret

        .globl _aim_direct_test_errno
_aim_direct_test_errno:
        sub sp, sp, #16
        str x30, [sp]
        mov x1, #0x101
        mov x2, #0x102
        mov x3, #0x103
        mov x4, #0x104
        mov x5, #0x105
        mov x6, #0x106
        mov x7, #0x107
        mov x8, #0x108
        mov x9, #0x109
        mov x10, #0x10a
        mov x11, #0x10b
        mov x12, #0x10c
        mov x13, #0x10d
        mov x14, #0x10e
        mov x15, #0x10f
        mov x16, #0x110
        mov x17, #0x111
        mov x18, #0x112
        mov x19, #0x113
        mov x20, #0x114
        mov x21, #0x115
        mov x22, #0x116
        mov x23, #0x117
        mov x24, #0x118
        mov x25, #0x119
        mov x26, #0x11a
        mov x27, #0x11b
        mov x28, #0x11c
        mov x29, #0x11d
        movi v0.16b, #0xaa
        movi v16.16b, #0xbb
        mov x9, #0xf000
        msr nzcv, x9
        adrp x0, _AIM_DIRECT_TEST_ERRNO@PAGE
        add x0, x0, _AIM_DIRECT_TEST_ERRNO@PAGEOFF
        ldr x30, [sp]
        add sp, sp, #16
        ret

        .globl _aim_direct_test_syscall_error
_aim_direct_test_syscall_error:
        sub sp, sp, #16
        str x30, [sp]
        mov x1, #0x201
        mov x2, #0x202
        mov x3, #0x203
        mov x4, #0x204
        mov x5, #0x205
        mov x6, #0x206
        mov x7, #0x207
        mov x8, #0x208
        mov x9, #0x209
        mov x10, #0x20a
        mov x11, #0x20b
        mov x12, #0x20c
        mov x13, #0x20d
        mov x14, #0x20e
        mov x15, #0x20f
        mov x16, #0x210
        mov x17, #0x211
        mov x18, #0x212
        mov x19, #0x213
        mov x20, #0x214
        mov x21, #0x215
        mov x22, #0x216
        mov x23, #0x217
        mov x24, #0x218
        mov x25, #0x219
        mov x26, #0x21a
        mov x27, #0x21b
        mov x28, #0x21c
        mov x29, #0x21d
        movi v0.16b, #0xcc
        movi v16.16b, #0xdd
        mov x9, #0x5000
        msr nzcv, x9
        adrp x9, _AIM_DIRECT_TEST_ERRNO@PAGE
        add x9, x9, _AIM_DIRECT_TEST_ERRNO@PAGEOFF
        mov w10, #5
        str w10, [x9]
        mov x0, #1
        neg x0, x0
        ldr x30, [sp]
        add sp, sp, #16
        ret

        .globl _aim_direct_test_syscall_success
_aim_direct_test_syscall_success:
        sub sp, sp, #16
        str x30, [sp]
        mov x1, #0x301
        mov x2, #0x302
        mov x3, #0x303
        mov x4, #0x304
        mov x5, #0x305
        mov x6, #0x306
        mov x7, #0x307
        mov x8, #0x308
        mov x9, #0x309
        mov x10, #0x30a
        mov x11, #0x30b
        mov x12, #0x30c
        mov x13, #0x30d
        mov x14, #0x30e
        mov x15, #0x30f
        mov x16, #0x310
        mov x17, #0x311
        mov x18, #0x312
        mov x19, #0x313
        mov x20, #0x314
        mov x21, #0x315
        mov x22, #0x316
        mov x23, #0x317
        mov x24, #0x318
        mov x25, #0x319
        mov x26, #0x31a
        mov x27, #0x31b
        mov x28, #0x31c
        mov x29, #0x31d
        movi v0.16b, #0xee
        movi v16.16b, #0xff
        mov x9, #0x3000
        msr nzcv, x9
        adrp x9, _AIM_DIRECT_TEST_ERRNO@PAGE
        add x9, x9, _AIM_DIRECT_TEST_ERRNO@PAGEOFF
        mov w10, #7
        str w10, [x9]
        mov x0, #123
        ldr x30, [sp]
        add sp, sp, #16
        ret
        "#
    );

    unsafe extern "C" {
        fn aim_direct_test_invoke(entry: usize, output: *mut Snapshot);
        fn aim_direct_test_errno() -> *mut i32;
        fn aim_direct_test_syscall_error() -> i64;
        fn aim_direct_test_syscall_success() -> i64;
    }

    fn movz(register: u32, immediate: u16) -> u32 {
        0xd280_0000 | (u32::from(immediate) << 5) | register
    }

    fn str_x(register: u32, offset: usize) -> u32 {
        0xf900_0000 | ((offset as u32 / 8) << 10) | (31 << 5) | register
    }

    fn ldr_x(register: u32, offset: usize) -> u32 {
        0xf940_0000 | ((offset as u32 / 8) << 10) | (31 << 5) | register
    }

    fn str_x_base(register: u32, base: u32, offset: usize) -> u32 {
        0xf900_0000 | ((offset as u32 / 8) << 10) | (base << 5) | register
    }

    fn str_q_base(register: u32, base: u32, offset: usize) -> u32 {
        0x3d80_0000 | ((offset as u32 / 16) << 10) | (base << 5) | register
    }

    fn ldr_q_base(register: u32, base: u32, offset: usize) -> u32 {
        0x3dc0_0000 | ((offset as u32 / 16) << 10) | (base << 5) | register
    }

    fn branch(from: usize, to: usize) -> u32 {
        let delta = to as i128 - from as i128;
        assert_eq!(delta & 3, 0);
        assert!((-(1_i128 << 27)..((1_i128 << 27) - 4)).contains(&delta));
        0x1400_0000 | ((((delta >> 2) as i64 as u64) as u32) & 0x03ff_ffff)
    }

    fn put(words: &mut Vec<u32>, word: u32) {
        words.push(word);
    }

    unsafe fn run_case(syscall_target: usize, expected: i64, old_errno: i32, resulting_errno: i32) {
        // SAFETY: this test owns the mapping, emits bounded code before RX,
        // and keeps the snapshot and mock targets alive for the invocation.
        unsafe {
            let page = mmap(
                ptr::null_mut(),
                PAGE_SIZE,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANON,
                -1,
                0,
            );
            assert_ne!(page as isize, -1);
            let code = page.cast::<u8>();
            let mut words = Vec::new();
            put(&mut words, 0xd108_03ff); // sub sp, sp, #0x200
            put(&mut words, str_x(0, ENTRY_STACK_POINTER));
            put(&mut words, str_x(30, ENTRY_SAVED_X30));
            for register in 1..=29 {
                let value = if register == 8 {
                    172
                } else {
                    0x1000 + register
                };
                put(&mut words, movz(register, value as u16));
            }
            let branch_index = words.len();
            put(&mut words, 0);
            let post_offset = words.len() * 4;
            put(&mut words, str_x(0, ENTRY_RESULT));
            for register in 1..=30 {
                put(
                    &mut words,
                    str_x(register, ENTRY_REGS + (register as usize - 1) * 8),
                );
            }
            put(
                &mut words,
                0x3d80_0000 | ((ENTRY_Q0 as u32 / 16) << 10) | (31 << 5),
            ); // str q0, [sp, #288]
            put(
                &mut words,
                0x3d80_0000 | ((ENTRY_Q16 as u32 / 16) << 10) | (31 << 5) | 16,
            ); // str q16, [sp, #304]
            put(&mut words, 0xd53b_4209); // mrs x9, nzcv
            put(&mut words, str_x(9, ENTRY_NZCV));
            put(&mut words, ldr_x(16, ENTRY_STACK_POINTER));
            put(&mut words, ldr_x(17, ENTRY_RESULT));
            put(&mut words, str_x_base(17, 16, 0));
            put(&mut words, ldr_x(17, ENTRY_SAVED_X30));
            put(&mut words, str_x_base(17, 16, 8));
            for register in 1..=30 {
                put(&mut words, ldr_x(17, ENTRY_REGS + (register - 1) * 8));
                put(&mut words, str_x_base(17, 16, 16 + (register - 1) * 8));
            }
            put(&mut words, ldr_q_base(0, 31, ENTRY_Q0));
            put(&mut words, str_q_base(0, 16, 256));
            put(&mut words, ldr_q_base(16, 31, ENTRY_Q16));
            put(&mut words, str_q_base(16, 16, 272));
            put(&mut words, ldr_x(17, ENTRY_NZCV));
            put(&mut words, str_x_base(17, 16, 288));
            put(&mut words, ldr_x(0, ENTRY_RESULT));
            put(&mut words, 0x9108_03ff); // add sp, sp, #0x200
            put(&mut words, 0xd65f_03c0); // ret

            let veneer_base = code as usize + VENEER_OFFSET;
            words[branch_index] = branch(code as usize + branch_index * 4, veneer_base);
            for (index, word) in words.iter().enumerate() {
                code.add(index * 4).cast::<u32>().write_unaligned(*word);
            }
            direct_syscall::write_veneer(
                code.add(VENEER_OFFSET),
                syscall_target,
                aim_direct_test_errno as usize,
                code as usize + post_offset,
            )
            .unwrap();
            assert_eq!(mprotect(page, PAGE_SIZE, PROT_READ | PROT_EXEC), 0);
            sys_icache_invalidate(page, PAGE_SIZE);

            ptr::addr_of_mut!(AIM_DIRECT_TEST_ERRNO).write(old_errno);
            let mut snapshot = Snapshot {
                result: 0,
                expected_x30: 0,
                registers: [0; 30],
                q0: [0; 16],
                q16: [0; 16],
                nzcv: 0,
            };
            aim_direct_test_invoke(code as usize, &mut snapshot);
            assert_eq!(snapshot.result, expected);
            assert_eq!(ptr::addr_of!(AIM_DIRECT_TEST_ERRNO).read(), resulting_errno);
            for (index, register) in snapshot.registers.iter().enumerate() {
                let register_number = index + 1;
                if register_number == 30 {
                    assert_ne!(*register, 0);
                } else if register_number == 8 {
                    assert_eq!(*register, 172);
                } else {
                    assert_eq!(*register, (0x1000 + register_number) as u64);
                }
            }
            assert_eq!(snapshot.q0, [0x11; 16]);
            assert_eq!(snapshot.q16, [0x22; 16]);
            assert_eq!(snapshot.nzcv, 0x6000_0000);
            assert_eq!(snapshot.expected_x30, snapshot.registers[29]);
            assert_eq!(munmap(page, PAGE_SIZE), 0);
        }
    }

    #[test]
    #[ignore = "requires SDK12 x18 task ABI; run tools/audit-direct-syscall.sh"]
    fn executable_veneer_preserves_registers_vectors_flags_and_errno() {
        unsafe {
            run_case(aim_direct_test_syscall_error as usize, -5, 38, 38);
            run_case(aim_direct_test_syscall_success as usize, 123, 41, 41);
        }
    }
}

#[cfg(not(all(target_arch = "aarch64", target_os = "macos")))]
#[test]
fn executable_veneer_register_regression_requires_macos_arm64() {}
