package org.apache.harmony.dalvik.ddmc;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;

/** Compile-only declaration; core-libart owns the runtime class. */
public abstract class ChunkHandler {
    public static final ByteOrder CHUNK_ORDER = ByteOrder.BIG_ENDIAN;
    public static final int CHUNK_FAIL = 0;

    public abstract void onConnected();
    public abstract void onDisconnected();
    public abstract Chunk handleChunk(Chunk request);
    public static Chunk createFailChunk(int errorCode, String message) {
        throw new AssertionError();
    }
    public static ByteBuffer wrapChunk(Chunk request) { throw new AssertionError(); }
    public static int type(String name) { throw new AssertionError(); }
    public static String name(int type) { throw new AssertionError(); }
}
