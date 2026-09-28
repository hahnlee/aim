package dev.aim.security;

import java.io.FileInputStream;
import java.io.IOException;
import java.security.ProviderException;
import java.security.SecureRandomSpi;

/** SecureRandom SPI backed by the Darwin kernel's /dev/urandom device. */
public final class DarwinSecureRandom extends SecureRandomSpi {
    private byte[] pendingSeed;

    @Override
    protected synchronized void engineSetSeed(byte[] seed) {
        if (seed == null || seed.length == 0) return;
        pendingSeed = seed.clone();
    }

    @Override
    protected synchronized void engineNextBytes(byte[] bytes) {
        if (bytes == null) throw new NullPointerException("bytes");
        try (FileInputStream input = new FileInputStream("/dev/urandom")) {
            int offset = 0;
            while (offset < bytes.length) {
                int count = input.read(bytes, offset, bytes.length - offset);
                if (count < 0) throw new IOException("unexpected entropy EOF");
                offset += count;
            }
        } catch (IOException error) {
            throw new ProviderException("Darwin kernel entropy unavailable", error);
        }
        if (pendingSeed != null) {
            for (int index = 0; index < bytes.length; ++index) {
                bytes[index] ^= pendingSeed[index % pendingSeed.length];
            }
            pendingSeed = null;
        }
    }

    @Override
    protected byte[] engineGenerateSeed(int byteCount) {
        if (byteCount < 0) throw new IllegalArgumentException("byteCount");
        byte[] seed = new byte[byteCount];
        engineNextBytes(seed);
        return seed;
    }
}
