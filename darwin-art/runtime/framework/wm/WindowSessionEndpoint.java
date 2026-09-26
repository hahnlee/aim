package dev.darwinart.runtime.wm;

import android.graphics.Rect;
import android.os.Binder;
import android.os.Parcel;
import android.os.Parcelable;
import android.os.RemoteException;
import android.view.InputChannel;
import android.view.WindowManager;
import java.util.concurrent.ConcurrentHashMap;

/** Per-process window-session Binder owned by the system window service. */
final class WindowSessionEndpoint extends Binder {
    private static final String DESCRIPTOR = "android.view.IWindowSession";
    private static final int ADD_FLAG_APP_VISIBLE = 1;
    private static final int ADD_FLAG_IN_TOUCH_MODE = 2;
    private static final int RELAYOUT_RES_FIRST_TIME = 2;
    private final WindowSurfaceRegistry surfaces;
    private final WindowSessionIdentity identity;
    private final WindowSessionWindowOwnership windows;
    private final WindowPublicationController publications;
    private final ConcurrentHashMap<WindowSessionWindowOwnership.Registration, InputChannel>
            serverInputChannels = new ConcurrentHashMap<>();
    private final int addToDisplayAsUserCode = transaction("addToDisplayAsUser");
    private final int removeCode = transaction("remove");
    private final int relayoutCode = transaction("relayout");

    WindowSessionEndpoint(WindowSurfaceRegistry surfaces, WindowSessionIdentity identity,
            WindowSessionWindowOwnership windows, WindowPublicationController publications) {
        this.identity = identity;
        this.windows = windows;
        this.publications = publications;
        this.surfaces = surfaces;
        attachInterface(null, DESCRIPTOR);
    }

    private static int transaction(String name) {
        try {
            java.lang.reflect.Field field = Class.forName("android.view.IWindowSession$Stub")
                    .getDeclaredField("TRANSACTION_" + name);
            field.setAccessible(true);
            return field.getInt(null);
        } catch (ReflectiveOperationException error) {
            throw new ExceptionInInitializerError(error);
        }
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (code != removeCode && code != relayoutCode && code != addToDisplayAsUserCode)
            return super.onTransact(code, data, reply, flags);
        identity.requireCaller(Binder.getCallingPid(), Binder.getCallingUid());
        if (code == removeCode) {
            data.enforceInterface(DESCRIPTOR);
            android.os.IBinder window = data.readStrongBinder();
            data.enforceNoDataAvail();
            WindowSessionWindowOwnership.Registration registration = windows.beginCleanup(this, window);
            try {
                identity.requireCaller(Binder.getCallingPid(), Binder.getCallingUid());
                windows.retire(this, registration);
                window = registration.window();
                publications.remove(registration);
                surfaces.remove(identity.pid(), window);
                serverInputChannels.remove(registration);
                windows.release(this, registration);
                reply.writeNoException();
                return true;
            } finally {
                windows.end(this, registration);
            }
        }
        if (code == relayoutCode) {
            data.enforceInterface(DESCRIPTOR);
            android.os.IBinder window = data.readStrongBinder();
            WindowManager.LayoutParams attrs =
                    data.readTypedObject(WindowManager.LayoutParams.CREATOR);
            int requestedWidth = data.readInt();
            int requestedHeight = data.readInt();
            int viewVisibility = data.readInt();
            data.readInt(); // relayout flags
            data.readInt(); // sequence
            int lastSyncSequenceId = data.readInt();
            data.enforceNoDataAvail();
            WindowSessionWindowOwnership.Registration registration = windows.begin(this, window);
            try {
                identity.requireCaller(Binder.getCallingPid(), Binder.getCallingUid());
                window = registration.window();
                InputChannel serverChannel = serverInputChannels.get(registration);
                if (serverChannel == null) throw new IllegalStateException("window has no input channel");
                publications.validateRelayout(registration, surfaces.effectiveAttributes(window, attrs));
                WindowSurfaceRegistry.RelayoutPublication publication =
                        surfaces.relayout(identity.pid(), window, attrs,
                        requestedWidth, requestedHeight, viewVisibility, lastSyncSequenceId);
                publications.relayout(registration, publication.frame,
                        publication.viewVisibility, publication.effectiveAttributes);
                reply.writeNoException();
                reply.writeInt(RELAYOUT_RES_FIRST_TIME);
                reply.writeInt(1);
                publication.result.writeToParcel(reply, Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
                return true;
            } finally {
                windows.end(this, registration);
            }
        }
        data.enforceInterface(DESCRIPTOR);
        android.os.IBinder window = data.readStrongBinder();
        WindowManager.LayoutParams attrs =
                data.readTypedObject(WindowManager.LayoutParams.CREATOR);
        int viewVisibility = data.readInt();
        int displayId = data.readInt();
        data.readInt(); // user id
        data.readInt(); // requested visible inset types
        int scaleArrayLength = data.readInt();
        data.enforceNoDataAvail();

        WindowSessionWindowOwnership.Registration registration = windows.claim(this, window);
        InputChannel[] channels = null;
        boolean committed = false;
        boolean adopted = false;
        Throwable failure = null;
        try {
            identity.requireCaller(Binder.getCallingPid(), Binder.getCallingUid());
            window = registration.window();
            channels = InputChannel.openInputChannelPair(
                    "darwin-art-window-"
                            + Integer.toHexString(System.identityHashCode(window)));
            surfaces.add(identity.pid(), window, attrs, viewVisibility);
            serverInputChannels.put(registration, channels[1]);
            publications.register(registration, identity, channels[1], attrs, displayId);
            adopted = true;
            if (System.getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != null) {
                android.util.Log.i("DarwinWindowSession", "add input channel window="
                        + Integer.toHexString(System.identityHashCode(window))
                        + " client=" + channels[0] + " server=" + channels[1]);
            }
            reply.writeNoException();
            reply.writeInt(ADD_FLAG_APP_VISIBLE | ADD_FLAG_IN_TOUCH_MODE);
            reply.writeInt(1);
            channels[0].writeToParcel(reply, Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
            reply.writeInt(0); // InsetsState: the built-in display has no decor sources.
            reply.writeInt(0); // InsetsSourceControl.Array
            reply.writeInt(1);
            // Attached frame: the owning task's current bounds.
            surfaces.taskBounds(identity.pid())
                    .writeToParcel(reply, Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
            float[] scale = new float[Math.max(0, scaleArrayLength)];
            if (scale.length > 0) scale[0] = 1.0f;
            reply.writeFloatArray(scale);
            windows.ready(this, registration);
            publications.ready(registration);
            committed = true;
            return true;
        } catch (RuntimeException | Error error) {
            failure = error;
            throw error;
        } finally {
            try {
                if (!committed) {
                    windows.retire(this, registration);
                    if (channels != null) {
                        boolean owned = adopted || publications.ownsOriginal(channels[1]);
                        if (owned) publications.remove(registration);
                        channels[0].dispose();
                        if (!owned) channels[1].dispose();
                    }
                    surfaces.remove(identity.pid(), registration.window());
                    serverInputChannels.remove(registration);
                    windows.release(this, registration);
                }
            } catch (RuntimeException | Error cleanupError) {
                // Keep the original registration if old resources are unresolved.
                if (failure == null) throw cleanupError;
                if (cleanupError != failure) failure.addSuppressed(cleanupError);
            } finally {
                windows.end(this, registration);
            }
        }
    }
}
