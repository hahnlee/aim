package dev.darwinart.runtime.recovery;

import android.os.Parcel;
import android.util.Log;

/** Every wipe, recovery reboot and update path is refused and reported. */
public final class RecoverySystemEndpointTest {
    private static void check(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }

    private static Parcel request(Object... arguments) {
        Parcel data = new Parcel();
        data.writeInterfaceToken(RecoverySystemEndpoint.DESCRIPTOR);
        for (Object argument : arguments) {
            if (argument instanceof String) data.writeString((String) argument);
            else if (argument instanceof Boolean) data.writeBoolean((Boolean) argument);
            else data.writeString("");
        }
        return data;
    }

    public static void main(String[] args) throws Exception {
        RecoverySystemEndpoint endpoint = new RecoverySystemEndpoint();
        int refusals = Log.refusals;

        // RecoverySystem.rebootWipeUserData -> bootCommand -> setupBcb.
        Parcel reply = new Parcel();
        check(endpoint.transact(2, request("--wipe_data\n--reason=test"), reply, 0),
                "setupBcb was not answered");
        check(reply.noException && !reply.readBoolean(), "a wipe command was acknowledged");
        check(Log.refusals == refusals + 1, "a refused wipe was not reported");

        try {
            endpoint.transact(4, request("--wipe_data"), new Parcel(), 0);
            throw new AssertionError("rebootRecoveryWithCommand was not refused");
        } catch (SecurityException expected) {
        }

        Parcel uncrypt = new Parcel();
        endpoint.transact(1, request("/data/ota.zip", "listener"), uncrypt, 0);
        check(!uncrypt.readBoolean(), "uncrypt was acknowledged");
        Parcel lskf = new Parcel();
        endpoint.transact(9, request("pkg", "reason", Boolean.TRUE), lskf, 0);
        check(lskf.readInt() != 0, "rebootWithLskf reported RESUME_ON_REBOOT_REBOOT_ERROR_NONE");
        Parcel clear = new Parcel();
        endpoint.transact(3, request(), clear, 0);
        check(clear.readBoolean(), "clearBcb with no control block failed");
        System.out.println("recovery-system: wipe, recovery reboot and update requests refused PASS");
    }
}
