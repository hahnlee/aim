package dev.darwinart.runtime.wm;

import android.os.Binder;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.IBinder;
import android.os.Parcel;
import android.os.RemoteException;
import android.util.Log;
import dev.darwinart.runtime.os.SystemServices;

/**
 * App-process transport between the AppKit root geometry provider and the
 * system ActivityTask geometry owner.
 *
 * <p>Host reports are drained by a dedicated thread that blocks in native
 * code, so AppKit never calls into Java. Task revisions arrive as one-way
 * Binder calls (ordered per receiver) and are applied to the exact process
 * root on the AppKit main thread; the result is reported back one-way.</p>
 */
public final class DesktopRootGeometryClient {
    private static final String TAG = "DarwinRootGeometry";
    private static final String SERVICE_NAME = "darwin.root_geometry";
    private static final Object LOCK = new Object();
    private static boolean registered;

    private DesktopRootGeometryClient() {}

    /** True when this process owns a visible desktop root. */
    private static native boolean nativeHasRoot();
    /** Blocks until a host report newer than {@code afterSerial}: {serial, width, height}. */
    private static native long[] nativeAwaitReport(long afterSerial);
    /** Applies one task revision to the process root; returns the host status. */
    private static native int nativeApply(long revision, long hostSerial, int androidWidth,
            int androidHeight, int pointsWidth, int pointsHeight);

    public static void register() throws RemoteException {
        synchronized (LOCK) {
            if (registered || !nativeHasRoot()) return;
            final IBinder service = SystemServices.getService(SERVICE_NAME);
            if (service == null) throw new RemoteException("desktop root geometry service unavailable");
            HandlerThread applyThread = new HandlerThread("DarwinRootGeometryApply");
            applyThread.setDaemon(true);
            applyThread.start();
            final Handler applier = new Handler(applyThread.getLooper());
            IBinder receiver = new Binder() {
                {
                    attachInterface(null, TaskGeometryController.HOST_RECEIVER_DESCRIPTOR);
                }

                @Override
                protected boolean onTransact(int code, Parcel data, Parcel reply, int flags) {
                    if (code != TaskGeometryController.HOST_PUBLISH) return false;
                    data.enforceInterface(TaskGeometryController.HOST_RECEIVER_DESCRIPTOR);
                    long revision = data.readLong();
                    long hostSerial = data.readLong();
                    int androidWidth = data.readInt();
                    int androidHeight = data.readInt();
                    int pointsWidth = data.readInt();
                    int pointsHeight = data.readInt();
                    data.enforceNoDataAvail();
                    // Never call out of a Binder transaction: the revision may
                    // be delivered nested inside register(). One serial thread
                    // applies revisions in arrival order and reports results.
                    boolean posted = applier.post(() -> applied(service, revision, nativeApply(
                            revision, hostSerial, androidWidth, androidHeight, pointsWidth,
                            pointsHeight)));
                    if (!posted) Log.e(TAG, "geometry revision dropped after shutdown " + revision);
                    return true;
                }
            };
            Parcel data = Parcel.obtain();
            Parcel reply = Parcel.obtain();
            try {
                data.writeInterfaceToken(DesktopRootGeometryEndpoint.DESCRIPTOR);
                data.writeStrongBinder(receiver);
                if (!service.transact(DesktopRootGeometryEndpoint.TRANSACTION_REGISTER,
                        data, reply, 0)) {
                    throw new RemoteException("desktop root geometry registration rejected");
                }
                reply.readException();
            } finally {
                reply.recycle();
                data.recycle();
            }
            registered = true;
            Log.i(TAG, "desktop root registered for task geometry");
            Thread reporter = new Thread(() -> drainReports(service), "DarwinRootGeometry");
            reporter.setDaemon(true);
            reporter.start();
        }
    }

    private static void drainReports(IBinder service) {
        long serial = 0;
        while (true) {
            long[] report = nativeAwaitReport(serial);
            if (report == null) return; // Root closed or process shutdown.
            serial = report[0];
            Parcel data = Parcel.obtain();
            Parcel reply = Parcel.obtain();
            try {
                data.writeInterfaceToken(DesktopRootGeometryEndpoint.DESCRIPTOR);
                data.writeLong(serial);
                data.writeInt((int) report[1]);
                data.writeInt((int) report[2]);
                // Synchronous: this dedicated thread holds no lock, and the
                // reply orders reports before the next latest-wins drain.
                if (!service.transact(DesktopRootGeometryEndpoint.TRANSACTION_REPORT, data,
                        reply, 0)) {
                    throw new RemoteException("desktop root geometry report rejected");
                }
                reply.readException();
            } catch (RemoteException | RuntimeException error) {
                Log.e(TAG, "system geometry owner unavailable", error);
                return;
            } finally {
                reply.recycle();
                data.recycle();
            }
        }
    }

    private static void applied(IBinder service, long revision, int status) {
        Parcel data = Parcel.obtain();
        Parcel reply = Parcel.obtain();
        try {
            data.writeInterfaceToken(DesktopRootGeometryEndpoint.DESCRIPTOR);
            data.writeLong(revision);
            data.writeInt(status);
            if (service.transact(DesktopRootGeometryEndpoint.TRANSACTION_APPLIED, data, reply, 0)) {
                reply.readException();
            }
        } catch (RemoteException | RuntimeException error) {
            Log.w(TAG, "geometry result not delivered revision=" + revision, error);
        } finally {
            reply.recycle();
            data.recycle();
        }
    }
}
