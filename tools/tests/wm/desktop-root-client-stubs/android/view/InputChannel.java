package android.view;

import android.os.IBinder;

/** Minimal fixture surface for the original per-window input channel token. */
public final class InputChannel {
    private IBinder token;

    public InputChannel(IBinder value) { token = value; }

    public IBinder getToken() { return token; }

    public void setTokenForTest(IBinder value) { token = value; }
}
