package dev.aim.server;

import android.os.Binder;
import android.os.IInterface;
import java.util.Objects;

/** Explicit synchronous capabilities returned by the authenticated native package owner. */
final class NativePackageCapabilities {
    private NativePackageCapabilities() {}
    static <T extends IInterface> T attach(T owner) {
        Objects.requireNonNull(owner, "native package capability unavailable");
        Binder.allowBlocking(owner.asBinder());
        return owner;
    }
}
