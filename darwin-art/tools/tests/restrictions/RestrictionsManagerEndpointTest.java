package dev.darwinart.runtime.restrictions;

import android.os.Bundle;
import android.os.IBinder;
import android.os.Parcel;

/** Focused wire-contract test for Android 16 IRestrictionsManager. */
public final class RestrictionsManagerEndpointTest {
    private static void check(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }

    private static Parcel applicationRestrictionsRequest() {
        Parcel data = Parcel.obtain();
        data.writeInterfaceToken(RestrictionsManagerEndpoint.DESCRIPTOR);
        data.writeString("org.example.app");
        return data;
    }

    private static Parcel providerRequest() {
        Parcel data = Parcel.obtain();
        data.writeInterfaceToken(RestrictionsManagerEndpoint.DESCRIPTOR);
        return data;
    }

    private static void testNoApplicationRestrictions() throws Exception {
        RestrictionsManagerEndpoint endpoint = new RestrictionsManagerEndpoint();
        Parcel reply = Parcel.obtain();
        check(endpoint.onTransact(RestrictionsManagerEndpoint.TRANSACTION_GET_APPLICATION_RESTRICTIONS,
                applicationRestrictionsRequest(), reply, 0),
                "getApplicationRestrictions transaction rejected");
        check(reply.hasNoException(), "successful reply omitted writeNoException");
        Bundle restrictions = reply.readTypedObject(Bundle.CREATOR);
        check(restrictions != null, "no application restrictions must be an empty Bundle");
        check(reply.dataAvail() == 0, "query reply contained trailing data");
    }

    private static void testNoProvider() throws Exception {
        RestrictionsManagerEndpoint endpoint = new RestrictionsManagerEndpoint();
        Parcel reply = Parcel.obtain();
        check(endpoint.onTransact(RestrictionsManagerEndpoint.TRANSACTION_HAS_RESTRICTIONS_PROVIDER,
                providerRequest(), reply, 0), "hasRestrictionsProvider transaction rejected");
        check(reply.hasNoException(), "successful reply omitted writeNoException");
        check(!reply.readBoolean(), "desktop policy must report no restrictions provider");
        check(reply.dataAvail() == 0, "provider reply contained trailing data");
    }

    private static void testValidationAndUnsupported() throws Exception {
        RestrictionsManagerEndpoint endpoint = new RestrictionsManagerEndpoint();
        Parcel wrongToken = Parcel.obtain();
        wrongToken.writeInterfaceToken("not.android.content.IRestrictionsManager");
        wrongToken.writeString("org.example.app");
        try {
            endpoint.onTransact(RestrictionsManagerEndpoint.TRANSACTION_GET_APPLICATION_RESTRICTIONS,
                    wrongToken, Parcel.obtain(), 0);
            throw new AssertionError("wrong interface token was accepted");
        } catch (SecurityException expected) {}

        Parcel trailing = applicationRestrictionsRequest();
        trailing.writeInt(7);
        try {
            endpoint.onTransact(RestrictionsManagerEndpoint.TRANSACTION_GET_APPLICATION_RESTRICTIONS,
                    trailing, Parcel.obtain(), 0);
            throw new AssertionError("unexpected argument was accepted");
        } catch (IllegalStateException expected) {}

        check(!endpoint.onTransact(IBinder.FIRST_CALL_TRANSACTION + 1,
                providerRequest(), Parcel.obtain(), 0),
                "unsupported transaction was accepted");
    }

    private static void testDescriptorAndIds() throws Exception {
        RestrictionsManagerEndpoint endpoint = new RestrictionsManagerEndpoint();
        Parcel descriptorReply = Parcel.obtain();
        check(endpoint.onTransact(IBinder.INTERFACE_TRANSACTION, Parcel.obtain(), descriptorReply, 0),
                "interface transaction rejected");
        check(RestrictionsManagerEndpoint.DESCRIPTOR.equals(descriptorReply.readString()),
                "wrong interface descriptor");
        check(RestrictionsManagerEndpoint.TRANSACTION_GET_APPLICATION_RESTRICTIONS == 1,
                "getApplicationRestrictions transaction drifted");
        check(RestrictionsManagerEndpoint.TRANSACTION_HAS_RESTRICTIONS_PROVIDER == 3,
                "hasRestrictionsProvider transaction drifted");
    }

    public static void main(String[] args) throws Exception {
        testNoApplicationRestrictions();
        testNoProvider();
        testValidationAndUnsupported();
        testDescriptorAndIds();
        System.out.println("restrictions-manager-endpoint: PASS");
    }
}
