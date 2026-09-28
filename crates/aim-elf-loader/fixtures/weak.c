extern int optional_provider(void) __attribute__((weak));

__attribute__((visibility("default"))) int weak_value(void) {
    return optional_provider == 0 ? 19 : optional_provider();
}
