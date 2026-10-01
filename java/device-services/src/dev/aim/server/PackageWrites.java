package dev.aim.server;

import android.content.Context;
import android.content.pm.PackageInstaller;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.Parcel;
import android.os.RemoteException;
import android.util.Slog;

/**
 * The original PackageManager's install sessions, told to the native
 * PackageManager's write model in the service host (docs/m4-packagemanager.md,
 * slice B): a session callback on a thread of its own sends each
 * session's parameters whenever they may have changed (created, badged,
 * opened or closed), which the installer does before it commits, and its
 * end. PackageManager offers no hook before a commit itself; the model
 * joins a session to the install it then sees in the package feed.
 */
final class PackageWrites extends PackageInstaller.SessionCallback {
    private static final String TAG = "AimPackageWrites";

    private final Context mContext;
    private final Object mLock = new Object();
    private Handler mHandler;
    private PackageInstaller mInstaller;
    /** The host, on the callback's thread. */
    private IPackageWritesHost mHost;

    PackageWrites(Context context) {
        mContext = context;
    }

    /** IBridge.watchPackageWrites: tells `host` of the sessions from now on. */
    void attach(IPackageWritesHost host) {
        synchronized (mLock) {
            if (mHandler == null) {
                HandlerThread thread = new HandlerThread(TAG);
                thread.start();
                mHandler = thread.getThreadHandler();
                mInstaller = mContext.getPackageManager().getPackageInstaller();
                mInstaller.registerSessionCallback(this, mHandler);
            }
            mHandler.post(() -> mHost = host);
        }
    }

    @Override
    public void onCreated(int sessionId) {
        send(sessionId);
    }

    @Override
    public void onBadgingChanged(int sessionId) {
        send(sessionId);
    }

    @Override
    public void onActiveChanged(int sessionId, boolean active) {
        send(sessionId);
    }

    @Override
    public void onProgressChanged(int sessionId, float progress) {
    }

    @Override
    public void onFinished(int sessionId, boolean success) {
        IPackageWritesHost host = mHost;
        if (host == null) {
            return;
        }
        try {
            host.finished(sessionId, success);
        } catch (RemoteException e) {
            lost(e);
        }
    }

    /** The session's parameters as they stand; none once it is gone. */
    private void send(int sessionId) {
        IPackageWritesHost host = mHost;
        PackageInstaller.SessionInfo info = mInstaller.getSessionInfo(sessionId);
        if (host == null || info == null) {
            return;
        }
        Parcel p = Parcel.obtain();
        try {
            p.writeString(info.getAppPackageName());
            p.writeInt(info.userId);
            p.writeInt(info.getMode());
            p.writeInt(info.installFlags);
            p.writeInt(info.getInstallReason());
            p.writeString(info.getInstallerPackageName());
            p.writeInt(info.getInstallerUid());
            p.writeInt(info.getOriginatingUid());
            p.writeInt(info.getPackageSource());
            p.writeBoolean(info.isApplicationEnabledSettingPersistent());
            p.writeBoolean(info.isMultiPackage());
            p.writeBoolean(info.isStaged());
            p.writeInt(info.getParentSessionId());
            p.writeBoolean(info.isCommitted());
            p.writeString(info.getResolvedBaseApkPath());
            host.session(sessionId, p.marshall());
        } catch (RemoteException e) {
            lost(e);
        } finally {
            p.recycle();
        }
    }

    private void lost(RemoteException e) {
        Slog.w(TAG, "the package writes' host died", e);
        mHost = null;
    }
}
