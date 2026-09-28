#include "asm_support_arm64_darwin.S"

    .extern _art_darwin_rust_callback

// Exercise the same ENTRY/END, CFI, frame, and external-call conventions used
// by ART's ARM64 native and quick entrypoints.
ENTRY art_darwin_asm_smoke
    SAVE_TWO_REGS_INCREASE_FRAME x19, xLR, 16
    CALL_SYMBOL art_darwin_rust_callback
    RESTORE_TWO_REGS_DECREASE_FRAME x19, xLR, 16
    add x0, x0, #1
    ret
END art_darwin_asm_smoke
