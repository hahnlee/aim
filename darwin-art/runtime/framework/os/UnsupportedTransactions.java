package dev.darwinart.runtime.os;

import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.os.Process;
import android.util.Log;
import java.lang.reflect.Method;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;

/**
 * The answer of this runtime's service endpoints to an AIDL transaction they
 * do not implement. Binder's unknown-transaction result stands, since much of
 * AOSP tolerates the empty reply, but the first such call per method is
 * logged with the interface and method name. For a system-server component
 * calling in process the log carries the caller's stack, so a missing
 * contract behind a later NullPointerException is found directly. With
 * {@code DARWIN_ART_STRICT_TRANSACTIONS=1} such an in-process caller gets an
 * {@link UnsupportedOperationException} at the call instead.
 */
public final class UnsupportedTransactions {
    private static final String TAG = "DarwinUnsupported";
    private static final Set<String> LOGGED = ConcurrentHashMap.newKeySet();
    private static final boolean STRICT =
            "1".equals(System.getenv("DARWIN_ART_STRICT_TRANSACTIONS"));

    private UnsupportedTransactions() {}

    /** True when this answered {@code code}; otherwise defer to Binder. */
    public static boolean reject(Binder endpoint, int code, Parcel reply, int flags) {
        if (code < IBinder.FIRST_CALL_TRANSACTION || code > IBinder.LAST_CALL_TRANSACTION) {
            return false; // PING, DUMP, INTERFACE, SHELL_COMMAND: Binder's own.
        }
        String descriptor = endpoint.getInterfaceDescriptor();
        String method = descriptor + "." + transactionName(descriptor, code);
        boolean oneway = (flags & IBinder.FLAG_ONEWAY) != 0;
        boolean inProcess = Binder.getCallingPid() == Process.myPid() && reply != null && !oneway;
        if (inProcess && STRICT) {
            reply.writeException(new UnsupportedOperationException(
                    method + " is not provided by this runtime"));
            return true;
        }
        if (LOGGED.add(method)) {
            if (inProcess) {
                // An in-process transaction runs on the caller's thread.
                Log.e(TAG, method + " is not provided by this runtime; the in-process"
                        + " caller reads an empty reply", new Throwable("caller"));
            } else {
                Log.w(TAG, method + " is not provided by this runtime (caller pid="
                        + Binder.getCallingPid() + ")");
            }
        }
        return false;
    }

    /** The AIDL method name ({@code Stub.getDefaultTransactionName}), or the code. */
    private static String transactionName(String descriptor, int code) {
        if (descriptor != null) {
            try {
                Method name = Class.forName(descriptor + "$Stub")
                        .getMethod("getDefaultTransactionName", int.class);
                Object value = name.invoke(null, code);
                if (value != null) return value.toString();
            } catch (ReflectiveOperationException | LinkageError ignored) {
                // Not an AIDL-generated interface.
            }
        }
        return "transaction#" + code;
    }
}
