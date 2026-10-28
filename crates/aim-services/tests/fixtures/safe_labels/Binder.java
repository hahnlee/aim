package android.os;

/** Host-only identity source; no Binder transport is simulated here. */
public abstract class Binder implements IBinder {
    public static int uid = 1000;
    public Binder() {}
    public void attachInterface(IInterface owner, String descriptor) {}
    public static int getCallingUid() { return uid; }
}
