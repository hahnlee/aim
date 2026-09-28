package android.os;

/** Test-only Message carrying the same fields used by ConnectivityManager.CallbackHandler. */
public final class Message {
    public int what;
    public int arg1;
    public int arg2;
    public Object obj;
    private Bundle data;

    public static Message obtain() { return new Message(); }

    public Bundle getData() {
        if (data == null) data = new Bundle();
        return data;
    }
}
