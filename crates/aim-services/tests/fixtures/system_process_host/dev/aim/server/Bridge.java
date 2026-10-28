package dev.aim.server;
final class Bridge {
    static boolean system = true;
    static void enforceSystemUid() { if (!system) throw new SecurityException(); }
}
