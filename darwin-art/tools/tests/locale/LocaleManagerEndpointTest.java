package dev.darwinart.runtime.locale;

import android.os.IBinder;
import android.os.LocaleList;
import android.os.Parcel;

/** Focused wire-contract test for Android 16 ILocaleManager. */
public final class LocaleManagerEndpointTest {
    private static void check(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }

    private static Parcel applicationLocalesRequest() {
        Parcel data = Parcel.obtain();
        data.writeInterfaceToken(LocaleManagerEndpoint.DESCRIPTOR);
        data.writeString("org.example.app");
        data.writeInt(0);
        return data;
    }

    private static void testEmptyApplicationLocales() throws Exception {
        LocaleManagerEndpoint endpoint = new LocaleManagerEndpoint();
        Parcel reply = Parcel.obtain();
        check(endpoint.onTransact(LocaleManagerEndpoint.TRANSACTION_GET_APPLICATION_LOCALES,
                applicationLocalesRequest(), reply, 0),
                "getApplicationLocales transaction rejected");
        check(reply.hasNoException(), "successful reply omitted writeNoException");
        LocaleList locales = reply.readTypedObject(LocaleList.CREATOR);
        check(locales != null && locales.isEmpty(),
                "missing per-app state must return an empty LocaleList");
        check(reply.dataAvail() == 0, "query reply contained trailing data");
    }

    private static void testSystemLocalesAreNeverNull() throws Exception {
        LocaleManagerEndpoint endpoint = new LocaleManagerEndpoint();
        Parcel data = Parcel.obtain();
        data.writeInterfaceToken(LocaleManagerEndpoint.DESCRIPTOR);
        Parcel reply = Parcel.obtain();
        check(endpoint.onTransact(LocaleManagerEndpoint.TRANSACTION_GET_SYSTEM_LOCALES,
                data, reply, 0), "getSystemLocales transaction rejected");
        check(reply.hasNoException(), "system locale reply omitted writeNoException");
        check(reply.readTypedObject(LocaleList.CREATOR) != null,
                "system locale query returned null");
        check(reply.dataAvail() == 0, "system locale reply contained trailing data");
    }

    private static void testValidationAndUnsupported() throws Exception {
        LocaleManagerEndpoint endpoint = new LocaleManagerEndpoint();
        Parcel wrongToken = applicationLocalesRequest();
        // Replace the token by putting a fresh request with the wrong descriptor.
        wrongToken = Parcel.obtain();
        wrongToken.writeInterfaceToken("not.android.app.ILocaleManager");
        wrongToken.writeString("org.example.app");
        wrongToken.writeInt(0);
        try {
            endpoint.onTransact(LocaleManagerEndpoint.TRANSACTION_GET_APPLICATION_LOCALES,
                    wrongToken, Parcel.obtain(), 0);
            throw new AssertionError("wrong interface token was accepted");
        } catch (SecurityException expected) {}

        Parcel trailing = applicationLocalesRequest();
        trailing.writeInt(7);
        try {
            endpoint.onTransact(LocaleManagerEndpoint.TRANSACTION_GET_APPLICATION_LOCALES,
                    trailing, Parcel.obtain(), 0);
            throw new AssertionError("unexpected argument was accepted");
        } catch (IllegalStateException expected) {}

        check(!endpoint.onTransact(LocaleManagerEndpoint.TRANSACTION_SET_APPLICATION_LOCALES,
                applicationLocalesRequest(), Parcel.obtain(), 0),
                "unsupported setter transaction was accepted");
    }

    private static void testDescriptorAndIds() throws Exception {
        LocaleManagerEndpoint endpoint = new LocaleManagerEndpoint();
        Parcel descriptorReply = Parcel.obtain();
        check(endpoint.onTransact(IBinder.INTERFACE_TRANSACTION, Parcel.obtain(), descriptorReply, 0),
                "interface transaction rejected");
        check(LocaleManagerEndpoint.DESCRIPTOR.equals(descriptorReply.readString()),
                "wrong interface descriptor");
        check(LocaleManagerEndpoint.TRANSACTION_SET_APPLICATION_LOCALES == 1,
                "setApplicationLocales transaction drifted");
        check(LocaleManagerEndpoint.TRANSACTION_GET_APPLICATION_LOCALES == 2,
                "getApplicationLocales transaction drifted");
        check(LocaleManagerEndpoint.TRANSACTION_GET_SYSTEM_LOCALES == 3,
                "getSystemLocales transaction drifted");
        check(LocaleManagerEndpoint.TRANSACTION_SET_OVERRIDE_LOCALE_CONFIG == 4,
                "setOverrideLocaleConfig transaction drifted");
        check(LocaleManagerEndpoint.TRANSACTION_GET_OVERRIDE_LOCALE_CONFIG == 5,
                "getOverrideLocaleConfig transaction drifted");
    }

    public static void main(String[] args) throws Exception {
        testEmptyApplicationLocales();
        testSystemLocalesAreNeverNull();
        testValidationAndUnsupported();
        testDescriptorAndIds();
        System.out.println("locale-manager-endpoint: PASS");
    }
}
