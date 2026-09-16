package dev.darwinart.runtime.inputmethod;

import android.os.Binder;
import android.os.Parcel;
import android.os.Parcelable;
import android.os.RemoteException;
import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.util.Collections;

/** Android input-method service identity for a hardware-keyboard-first device. */
public final class InputMethodManagerEndpoint extends Binder {
    private static final String DESCRIPTOR = "com.android.internal.view.IInputMethodManager";
    private final int getInputMethodListCode = transaction("getInputMethodList");
    private final int getEnabledInputMethodListCode = transaction("getEnabledInputMethodList");
    private final int getInputMethodListLegacyCode = transaction("getInputMethodListLegacy");
    private final int getEnabledInputMethodListLegacyCode =
            transaction("getEnabledInputMethodListLegacy");

    public InputMethodManagerEndpoint() {
        attachInterface(null, DESCRIPTOR);
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (code != getInputMethodListCode && code != getEnabledInputMethodListCode
                && code != getInputMethodListLegacyCode
                && code != getEnabledInputMethodListLegacyCode) {
            return super.onTransact(code, data, reply, flags);
        }
        if (reply == null) return false;
        data.enforceInterface(DESCRIPTOR);
        data.readInt(); // userId
        if (code == getInputMethodListCode || code == getInputMethodListLegacyCode) {
            data.readInt(); // directBootAwareness
        }
        data.enforceNoDataAvail();
        reply.writeNoException();
        if (code == getInputMethodListCode || code == getEnabledInputMethodListCode) {
            reply.writeTypedObject(emptySafeList(), Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
        } else {
            reply.writeTypedList(Collections.<Parcelable>emptyList());
        }
        return true;
    }

    private static int transaction(String name) {
        try {
            Field field = Class.forName("com.android.internal.view.IInputMethodManager$Stub")
                    .getDeclaredField("TRANSACTION_" + name);
            field.setAccessible(true);
            return field.getInt(null);
        } catch (ReflectiveOperationException error) {
            throw new ExceptionInInitializerError(error);
        }
    }

    private static Parcelable emptySafeList() {
        try {
            Class<?> type = Class.forName(
                    "com.android.internal.inputmethod.InputMethodInfoSafeList");
            Method empty = type.getDeclaredMethod("empty");
            empty.setAccessible(true);
            return (Parcelable) empty.invoke(null);
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("InputMethodInfoSafeList.empty unavailable", error);
        }
    }
}
