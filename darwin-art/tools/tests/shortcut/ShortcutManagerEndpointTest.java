package dev.darwinart.runtime.shortcut;

import android.content.pm.ParceledListSlice;
import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import dev.darwinart.runtime.am.ApplicationProcessRegistry;

/** Focused host-Java tests for the Android 16 shortcut Binder contract. */
public final class ShortcutManagerEndpointTest {
    private static void check(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }

    private static ApplicationProcessRegistry identifiedProcess() {
        ApplicationProcessRegistry processes = new ApplicationProcessRegistry();
        processes.beginAttachment(42, 10042, new Binder(), 1L);
        processes.identify(42, "org.example.app");
        processes.finishAttachment(42, 1L);
        Binder.setCallingPid(42);
        return processes;
    }

    private static Parcel shortcutsRequest(String packageName) {
        Parcel data = Parcel.obtain();
        data.writeInterfaceToken(ShortcutManagerEndpoint.DESCRIPTOR);
        data.writeString(packageName);
        data.writeInt(1); // matchFlags
        data.writeInt(0); // userId
        return data;
    }

    private static void testGetShortcutsReturnsFrameworkEmptySlice() throws Exception {
        ShortcutManagerEndpoint endpoint =
                new ShortcutManagerEndpoint(identifiedProcess());
        Parcel reply = Parcel.obtain();
        check(endpoint.onTransact(ShortcutManagerEndpoint.TRANSACTION_GET_SHORTCUTS,
                shortcutsRequest("org.example.app"), reply, 0),
                "getShortcuts transaction returned false");
        check(reply.hasNoException(), "successful reply omitted writeNoException");
        ParceledListSlice result = reply.readTypedObject(ParceledListSlice.CREATOR);
        check(result == ParceledListSlice.emptyList(),
                "getShortcuts did not return framework-owned emptyList()");
        check(result.isEmpty(), "getShortcuts returned a non-empty slice");
        check(reply.dataAvail() == 0, "query reply contained trailing data");
    }

    private static void testReportShortcutUsedRemainsSupported() throws Exception {
        ShortcutManagerEndpoint endpoint =
                new ShortcutManagerEndpoint(identifiedProcess());
        Parcel data = Parcel.obtain();
        data.writeInterfaceToken(ShortcutManagerEndpoint.DESCRIPTOR);
        data.writeString("org.example.app");
        data.writeString("shortcut-id");
        data.writeInt(0);
        Parcel reply = Parcel.obtain();
        check(endpoint.onTransact(ShortcutManagerEndpoint.TRANSACTION_REPORT_SHORTCUT_USED,
                data, reply, 0), "reportShortcutUsed transaction returned false");
        check(reply.hasNoException(), "report reply omitted writeNoException");
        check(reply.dataAvail() == 0, "report reply contained trailing data");
    }

    private static void testOwnershipAndWireValidation() throws Exception {
        ShortcutManagerEndpoint endpoint =
                new ShortcutManagerEndpoint(identifiedProcess());
        try {
            endpoint.onTransact(ShortcutManagerEndpoint.TRANSACTION_GET_SHORTCUTS,
                    shortcutsRequest("org.other.app"), Parcel.obtain(), 0);
            throw new AssertionError("mismatched package was accepted");
        } catch (SecurityException expected) {}

        Parcel wrongToken = shortcutsRequest("org.example.app");
        wrongToken = Parcel.obtain();
        wrongToken.writeInterfaceToken("not.android.content.pm.IShortcutService");
        wrongToken.writeString("org.example.app");
        wrongToken.writeInt(1);
        wrongToken.writeInt(0);
        try {
            endpoint.onTransact(ShortcutManagerEndpoint.TRANSACTION_GET_SHORTCUTS,
                    wrongToken, Parcel.obtain(), 0);
            throw new AssertionError("wrong interface token was accepted");
        } catch (SecurityException expected) {}

        Parcel trailing = shortcutsRequest("org.example.app");
        trailing.writeInt(7);
        try {
            endpoint.onTransact(ShortcutManagerEndpoint.TRANSACTION_GET_SHORTCUTS,
                    trailing, Parcel.obtain(), 0);
            throw new AssertionError("unexpected argument was accepted");
        } catch (IllegalStateException expected) {}
    }

    private static void testDescriptorAndUnsupportedTransactions() throws Exception {
        ShortcutManagerEndpoint endpoint =
                new ShortcutManagerEndpoint(identifiedProcess());
        Parcel descriptorReply = Parcel.obtain();
        check(endpoint.onTransact(IBinder.INTERFACE_TRANSACTION, Parcel.obtain(), descriptorReply, 0),
                "interface transaction returned false");
        check(ShortcutManagerEndpoint.DESCRIPTOR.equals(descriptorReply.readString()),
                "interface descriptor changed");
        check(!endpoint.onTransact(ShortcutManagerEndpoint.TRANSACTION_GET_SHORTCUTS + 1,
                shortcutsRequest("org.example.app"), Parcel.obtain(), 0),
                "unsupported transaction was accepted");
    }

    public static void main(String[] args) throws Exception {
        check(ShortcutManagerEndpoint.TRANSACTION_REPORT_SHORTCUT_USED == 14,
                "Android 16 reportShortcutUsed transaction changed");
        check(ShortcutManagerEndpoint.TRANSACTION_GET_SHORTCUTS == 23,
                "Android 16 getShortcuts transaction changed");
        testGetShortcutsReturnsFrameworkEmptySlice();
        testReportShortcutUsedRemainsSupported();
        testOwnershipAndWireValidation();
        testDescriptorAndUnsupportedTransactions();
        System.out.println("shortcut-manager-endpoint: PASS");
    }
}
