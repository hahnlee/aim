static int constructor_state;

__attribute__((constructor(101))) static void first_constructor(void) {
    constructor_state = 10;
}

__attribute__((constructor(102))) static void second_constructor(void) {
    constructor_state = constructor_state * 3 + 5;
}

__attribute__((visibility("default"))) int fixture_value(void) {
    return constructor_state + 7;
}

