package dev.aim.server;

import android.os.Binder;
import android.os.ServiceSpecificException;
import com.android.server.locksettings.recoverablekeystore.storage.RecoverableKeyStoreDb;
import java.util.ArrayList;
import java.util.List;

/** RecoverableKeyStoreManager generateKey/removeKey at android-16.0.0_r1. */
public final class RecoverableKeysTest {
    private static final List<String> events = new ArrayList<>();
    public static String failure = "";
    public static void event(String value) {
        if (Binder.uid != 1000) throw new AssertionError("keystore owner identity");
        events.add(value);
    }
    private static void expect(String... expected) {
        if (!events.equals(List.of(expected))) throw new AssertionError(events);
        events.clear();
        if (Binder.uid != 1000) throw new AssertionError("identity not restored");
    }
    private static void error(int code, String message, Call call) throws Exception {
        try { call.run(); throw new AssertionError("missing service error"); }
        catch (ServiceSpecificException e) {
            if (e.errorCode != code || !message.equals(e.getMessage())) throw new AssertionError(e);
        }
    }
    interface Call {void run() throws Exception;}
    public static void main(String[] args) throws Exception {
        var keys = new RecoverableKeyStoreKeys(new android.content.Context());
        // Untrusted callers cannot supply another UID, even for null aliases; no construction.
        Binder.uid=10123;
        for (Call call : List.<Call>of(() -> keys.generateKey(1000,null), () -> keys.removeKey(1000,null))) {
            try {call.run();throw new AssertionError("bridge allowed app UID");}
            catch (SecurityException expected) {}
            if (Binder.uid!=10123 || !events.isEmpty()) throw new AssertionError(events);
        }
        Binder.uid=1000;
        for (Call call : List.<Call>of(() -> keys.generateKey(10123,null), () -> keys.removeKey(10123,null))) {
            try {call.run();throw new AssertionError("null alias accepted");}
            catch (NullPointerException e) {if (!"alias is null".equals(e.getMessage())) throw e;}
        }
        expect("provider","open","verify","register:0:10123","register:0:10123");
        // Platform errors occur before generating/wrapping/importing a key or issuing a grant.
        for (String mode : List.of("insecure","keystore","io")) {
            failure=mode;
            int code=mode.equals("insecure") ? 23 : 22;
            String message=switch(mode){case "insecure" -> "Screenlock is not set";
                case "keystore" -> "key store failed"; default -> "database failed";};
            error(code,message,() -> keys.generateKey(10123,"a"));
            expect("register:0:10123","platform:0");
            if (!RecoverableKeyStoreDb.keys.isEmpty()) throw new AssertionError("failed generation persisted a key");
        }
        failure="remote";
        try {keys.generateKey(10123,"a");throw new AssertionError("remote failure swallowed");}
        catch (android.os.RemoteException expected) {}
        expect("register:0:10123","platform:0");
        failure="";
        if (!"recoverable_key:000000000000CAFE".equals(keys.generateKey(10123,"a"))) throw new AssertionError();
        expect("register:0:10123","platform:0","wrap:0:10123:a","store:0:10123:a","grant:0:10123:a");
        // A different recovery agent cannot remove this agent's key.
        keys.removeKey(10124,"a");
        expect("register:0:10124","remove:10124:a");
        keys.removeKey(10123,"a");
        expect("register:0:10123","remove:10123:a","snapshot:0:10123","delete:0:10123:a");
        keys.removeKey(10123,"a");
        expect("register:0:10123","remove:10123:a");
        // Explicit caller identity supplies both user and UID, independent of the system bridge.
        keys.generateKey(1010123,"");
        expect("register:10:1010123","platform:10","wrap:10:1010123:","store:10:1010123:","grant:10:1010123:");
        keys.removeKey(1010123,"");
        expect("register:10:1010123","remove:1010123:","snapshot:10:1010123","delete:10:1010123:");
        failure="wrap";
        error(22,"wrap failed",() -> keys.generateKey(10123,"a"));
        expect("register:0:10123","platform:0","wrap:0:10123:a");
        failure="store";
        error(22,"store failed",() -> keys.generateKey(10123,"a"));
        expect("register:0:10123","platform:0","wrap:0:10123:a","store:0:10123:a");
        // Original ordering leaves the wrapped row if AndroidKeyStore import fails.
        if (!RecoverableKeyStoreDb.keys.contains("10123:a")) throw new AssertionError("unexpected rollback");
        failure="delete";
        error(22,"delete failed",() -> keys.removeKey(10123,"a"));
        expect("register:0:10123","remove:10123:a","snapshot:0:10123","delete:0:10123:a");
        keys.removeKey(10123,"a");
        expect("register:0:10123","remove:10123:a");
    }
}
