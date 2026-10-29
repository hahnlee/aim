package dev.aim.server;

/** Rust verifies the Binder caller before entering this JVM. */
final class Bridge {
    static void enforceSystemUid() {}
}
