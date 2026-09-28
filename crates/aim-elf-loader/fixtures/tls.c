__thread int fixture_tls_state __attribute__((visibility("protected"))) = 7;
static __thread int fixture_tls_zero;
static __thread unsigned char fixture_tls_aligned[64]
    __attribute__((aligned(64)));

__attribute__((visibility("default"))) int fixture_tls(void) {
    return fixture_tls_state;
}

__attribute__((visibility("default"))) int fixture_tls_exchange(int value) {
    int result = fixture_tls_state * 1000 + fixture_tls_zero;
    fixture_tls_state = value;
    fixture_tls_zero += value;
    fixture_tls_aligned[0] = (unsigned char)value;
    return result;
}

__attribute__((visibility("default"))) int fixture_tls_alignment(void) {
    return (int)((unsigned long)&fixture_tls_aligned[0] & 63ul);
}

__attribute__((visibility("default"))) void *fixture_tls_descriptor(void) {
    void *descriptor;
    __asm__("adrp %0, :tlsdesc:fixture_tls_state\n\t"
            "add %0, %0, :tlsdesc_lo12:fixture_tls_state"
            : "=r"(descriptor));
    return descriptor;
}
