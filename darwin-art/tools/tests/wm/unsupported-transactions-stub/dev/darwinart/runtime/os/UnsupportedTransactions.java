package dev.darwinart.runtime.os;

import android.os.Binder;
import android.os.Parcel;

/**
 * Test stub: endpoint tests exercise implemented transactions, so every
 * unimplemented one keeps Binder's own result.
 */
public final class UnsupportedTransactions {
    private UnsupportedTransactions() {}

    public static boolean reject(Binder endpoint, int code, Parcel reply, int flags) {
        return false;
    }
}
