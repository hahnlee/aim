package android.view;

public final class InputChannel {
    private final android.os.IBinder token = new android.os.Binder();
    public android.os.IBinder getToken() { return token; }
    public final String name;
    public boolean disposed;

    public InputChannel(String name) {
        this.name = name;
    }

    public void dispose() { disposed = true; }
}
