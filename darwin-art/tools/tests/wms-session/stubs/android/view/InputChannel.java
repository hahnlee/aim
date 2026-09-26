package android.view;

import android.os.Parcel;

public final class InputChannel {
    private static int opened;
    private static int disposed;
    private boolean isDisposed;
    private final String name;
    private final android.os.IBinder token;

    public InputChannel(String name) { this(name, new android.os.Binder()); }
    private InputChannel(String name, android.os.IBinder token) { this.name = name; this.token = token; }
    public android.os.IBinder getToken() { return token; }
    public static void resetStats() { opened=0; disposed=0; }
    public static int openedCount() { return opened; }
    public static int disposedCount() { return disposed; }
    public static InputChannel[] openInputChannelPair(String name) {
        ++opened;
        android.os.IBinder token = new android.os.Binder();
        return new InputChannel[] { new InputChannel(name + ":client", token), new InputChannel(name + ":server", token) };
    }
    public void dispose() { if (!isDisposed) { isDisposed=true; ++disposed; } }
    public void writeToParcel(Parcel dest, int flags) { dest.writeString(name); }
}
