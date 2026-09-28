package org.apache.harmony.dalvik.ddmc;

import java.nio.ByteBuffer;

/** Compile-only declaration; core-libart owns the runtime class. */
public class Chunk {
    public int type;
    public byte[] data;
    public int offset;
    public int length;

    public Chunk() {}
    public Chunk(int type, byte[] data, int offset, int length) {}
    public Chunk(int type, ByteBuffer data) {}
}
