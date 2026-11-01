package dev.aim.server;

import android.content.Context;
import android.os.*;
import android.view.*;
import java.util.Objects;

/** IMS.setInputFilter / InputFilterHost, android-16.0.0_r1 (AOSP Apache-2.0). */
final class InputFilters {
    private final Context context;
    private final IAccessibilityWindowHost nativeHost;
    private IInputFilter filter;
    private Host host;
    InputFilters(Context context, IAccessibilityWindowHost nativeHost) {
        this.context = context; this.nativeHost = nativeHost;
    }
    synchronized void set(IInputFilter next) throws RemoteException {
        if (filter == next) return;
        if (filter != null) {
            host.disconnected = true;
            try { filter.uninstall(); } catch (RemoteException ignored) { /* IMS ignores remote death. */ }
        }
        filter = next;
        host = next == null ? null : new Host();
        if (next != null) {
            try { next.install(host); } catch (RemoteException ignored) { /* IMS ignores remote death. */ }
        }
        final Host generation = host;
        nativeHost.setInputFilter(next == null ? null : new IInputFilterCallback.Stub() {
            @Override public void filterInputEvent(byte[] bytes, int flags) throws RemoteException {
                synchronized (InputFilters.this) {
                    if (host != generation) return; // A queued callback from a disconnected generation.
                    Parcel parcel = Parcel.obtain();
                    try {
                        parcel.unmarshall(bytes, 0, bytes.length); parcel.setDataPosition(0);
                        next.filterInputEvent(InputEvent.CREATOR.createFromParcel(parcel), flags);
                    } finally { parcel.recycle(); }
                }
            }
        });
    }
    private final class Host extends IInputFilterHost.Stub {
        boolean disconnected;
        @Override public void sendInputEvent(InputEvent event, int flags) throws RemoteException {
            context.enforceCallingOrSelfPermission("android.permission.INJECT_EVENTS", "sendInputEvent()");
            Objects.requireNonNull(event, "event must not be null");
            synchronized (InputFilters.this) {
                if (disconnected) return;
                Parcel parcel = Parcel.obtain();
                try {
                    event.writeToParcel(parcel, 0);
                    nativeHost.sendInputEvent(parcel.marshall(), flags | 0x04000000 /* FLAG_FILTERED */);
                } finally { parcel.recycle(); }
            }
        }
    }
}
