package dev.darwinart.runtime.wm;

import android.os.Binder;
import android.os.Parcel;
import android.os.RemoteException;
import dev.darwinart.runtime.am.ApplicationProcessRegistry;
import dev.darwinart.runtime.display.TaskDisplayRegistry;

/** System-process owner for the pinned Android 16 IWindowManager contract. */
public final class WindowManagerEndpoint extends Binder {
    private static final String DESCRIPTOR = "android.view.IWindowManager";
    private final ApplicationProcessRegistry processes;
    private final WindowSurfaceRegistry surfaces;
    private final WindowSessionWindowOwnership windows = new WindowSessionWindowOwnership();
    private final WindowPublicationController publications = new WindowPublicationController();
    private final int openSessionCode = transaction("openSession");
    private final int hasNavigationBarCode = transaction("hasNavigationBar");
    private final int getCurrentAnimatorScaleCode = transaction("getCurrentAnimatorScale");

    public WindowManagerEndpoint(ApplicationProcessRegistry processes,
            DesktopWindowMetadataRegistry metadata, TaskDisplayRegistry displays) {
        if (processes == null || metadata == null || displays == null) {
            throw new NullPointerException();
        }
        this.processes = processes;
        surfaces = new WindowSurfaceRegistry(metadata, displays);
        attachInterface(null, DESCRIPTOR);
    }

    /** The WMS-wide window layout owner used by task geometry dispatch. */
    WindowSurfaceRegistry surfaces() {
        return surfaces;
    }

    /** Creates the root registry joined to this endpoint's publication owner. */
    public DesktopRootRegistry createDesktopRootRegistry() {
        return new DesktopRootRegistry(publications.bindingOwner());
    }

    DesktopRootRegistry createDesktopRootRegistry(DesktopForegroundAuthority.Provider authority) {
        return new DesktopRootRegistry(publications.bindingOwner(), authority);
    }

    private static int transaction(String name) {
        try {
            java.lang.reflect.Field field = Class.forName("android.view.IWindowManager$Stub")
                    .getDeclaredField("TRANSACTION_" + name);
            field.setAccessible(true);
            return field.getInt(null);
        } catch (ReflectiveOperationException error) {
            throw new ExceptionInInitializerError(error);
        }
    }

    /**
     * WindowManagerService.getCurrentAnimatorScale: the user's
     * Settings.Global.ANIMATOR_DURATION_SCALE (1 when unset).
     */
    private static float currentAnimatorScale() {
        android.app.ActivityThread thread = android.app.ActivityThread.currentActivityThread();
        android.content.Context context = thread == null ? null : thread.getSystemContext();
        if (context == null) return 1.0f;
        return android.provider.Settings.Global.getFloat(context.getContentResolver(),
                android.provider.Settings.Global.ANIMATOR_DURATION_SCALE, 1.0f);
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (code == openSessionCode) {
            data.enforceInterface(DESCRIPTOR);
            data.readStrongBinder(); // IWindowSessionCallback
            data.enforceNoDataAvail();
            WindowSessionIdentity identity = new WindowSessionIdentity(processes,
                    Binder.getCallingPid(), Binder.getCallingUid());
            WindowSessionEndpoint session = new WindowSessionEndpoint(surfaces, identity, windows, publications);
            reply.writeNoException();
            reply.writeStrongBinder(session);
            return true;
        }
        if (code == getCurrentAnimatorScaleCode) {
            data.enforceInterface(DESCRIPTOR);
            data.enforceNoDataAvail();
            reply.writeNoException();
            reply.writeFloat(currentAnimatorScale());
            return true;
        }
        if (code != hasNavigationBarCode) return dev.darwinart.runtime.os.UnsupportedTransactions.reject(this, code, reply, flags)
                || super.onTransact(code, data, reply, flags);
        data.enforceInterface(DESCRIPTOR);
        int displayId = data.readInt();
        data.enforceNoDataAvail();
        reply.writeNoException();
        // The Metal composer currently exposes no separate Android navigation
        // bar layer. Display 0 is therefore a decor-free built-in display.
        reply.writeBoolean(false);
        return true;
    }
}
