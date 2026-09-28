extern int provider_value(void);
extern int provider_data;

static int (*provider_slot)(void) = provider_value;
static volatile int relro_write_guard __attribute__((section(".data.rel.ro"))) = 7;

__attribute__((visibility("default"))) int imported_value(void) {
    return provider_value() + provider_slot() + provider_data;
}

__attribute__((visibility("default"))) int relro_write_attempt(void) {
    relro_write_guard = 9;
    return relro_write_guard;
}
