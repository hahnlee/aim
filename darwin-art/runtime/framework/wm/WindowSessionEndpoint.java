package dev.darwinart.runtime.wm;

import android.graphics.Rect;
import android.os.Binder;
import android.os.Parcel;
import android.os.Parcelable;
import android.os.RemoteException;
import android.view.InputChannel;
import android.view.WindowManager;
import dev.darwinart.runtime.display.BuiltInDisplayConfiguration;
import java.util.HashMap;

/** Per-process window-session Binder owned by the system window service. */
final class WindowSessionEndpoint extends Binder {
    private static final String DESCRIPTOR = "android.view.IWindowSession";
    private static final int ADD_FLAG_APP_VISIBLE = 1;
    private static final int ADD_FLAG_IN_TOUCH_MODE = 2;
    private static final int RELAYOUT_RES_FIRST_TIME = 2;
    private final WindowSurfaceRegistry surfaces;
    private final HashMap<android.os.IBinder, InputChannel> serverInputChannels =
            new HashMap<>();
    private final int addToDisplayAsUserCode = transaction("addToDisplayAsUser");
    private final int removeCode = transaction("remove");
    private final int relayoutCode = transaction("relayout");

    WindowSessionEndpoint(DesktopWindowMetadataRegistry metadata) {
        surfaces = new WindowSurfaceRegistry(metadata);
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
        if (code == removeCode) {
            data.enforceInterface(DESCRIPTOR);
            android.os.IBinder window = data.readStrongBinder();
            data.enforceNoDataAvail();
            InputChannel serverChannel = serverInputChannels.remove(window);
            if (serverChannel != null) {
                WindowInputPublisher.remove(serverChannel);
                serverChannel.dispose();
            }
            surfaces.remove(Binder.getCallingPid(), window);
            reply.writeNoException();
            return true;
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
            Parcelable result = surfaces.relayout(Binder.getCallingPid(), window, attrs,
                    requestedWidth, requestedHeight, viewVisibility, lastSyncSequenceId);
            WindowInputPublisher.publish(serverInputChannels.get(window), surfaces.frame(window));
            reply.writeNoException();
            reply.writeInt(RELAYOUT_RES_FIRST_TIME);
            reply.writeInt(1);
            result.writeToParcel(reply, Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
            return true;
        }
        if (code != addToDisplayAsUserCode) return super.onTransact(code, data, reply, flags);
        data.enforceInterface(DESCRIPTOR);
        android.os.IBinder window = data.readStrongBinder();
        WindowManager.LayoutParams attrs =
                data.readTypedObject(WindowManager.LayoutParams.CREATOR);
        int viewVisibility = data.readInt();
        data.readInt(); // layer stack id
        data.readInt(); // user id
        data.readInt(); // requested visible inset types
        int scaleArrayLength = data.readInt();
        data.enforceNoDataAvail();

        surfaces.add(Binder.getCallingPid(), window, attrs, viewVisibility);

        InputChannel oldServerChannel = serverInputChannels.remove(window);
        if (oldServerChannel != null) oldServerChannel.dispose();
        InputChannel[] channels;
        try {
            channels = InputChannel.openInputChannelPair(
                    "darwin-art-window-"
                            + Integer.toHexString(System.identityHashCode(window)));
        } catch (java.io.IOException error) {
            throw new IllegalStateException("cannot create Android input channel", error);
        }
        serverInputChannels.put(window, channels[1]);
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
        new Rect(0, 0, BuiltInDisplayConfiguration.WIDTH_PIXELS,
                BuiltInDisplayConfiguration.HEIGHT_PIXELS)
                .writeToParcel(reply, Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
        float[] scale = new float[Math.max(0, scaleArrayLength)];
        if (scale.length > 0) scale[0] = 1.0f;
        reply.writeFloatArray(scale);
        return true;
    }
}
