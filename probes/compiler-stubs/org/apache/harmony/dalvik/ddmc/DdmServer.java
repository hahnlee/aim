package org.apache.harmony.dalvik.ddmc;

/** Compile-only declaration; core-libart owns the runtime class. */
public final class DdmServer {
    public static void registerHandler(int type, ChunkHandler handler) {
        throw new AssertionError();
    }
    public static ChunkHandler unregisterHandler(int type) {
        throw new AssertionError();
    }
    public static void registrationComplete() { throw new AssertionError(); }
    public static void sendChunk(Chunk chunk) { throw new AssertionError(); }
}
