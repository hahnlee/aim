package dev.darwinart.runtime.wm;

import android.content.ComponentName;
import android.content.Intent;
import android.content.pm.ActivityInfo;
import android.os.Binder;
import android.os.Bundle;
import android.os.IBinder;
import android.os.Parcel;
import android.os.Parcelable;
import android.os.RemoteException;
import dev.darwinart.runtime.am.ApplicationProcessRegistry;
import dev.darwinart.runtime.pm.InstalledActivityInfo;
import dev.darwinart.runtime.pm.PackageRecords;
import java.lang.reflect.Field;

/** ActivityTaskManager Binder root; unsupported transactions fail explicitly. */
public final class ActivityTaskManagerEndpoint extends Binder {
    private final int getActivityClientControllerCode =
            transaction("getActivityClientController");
    private final int startActivityCode = transaction("startActivity");
    private final ActivityClientControllerEndpoint activityClient =
            new ActivityClientControllerEndpoint();
    private final PackageRecords.Source packages;
    private final ApplicationProcessRegistry processes;

    public ActivityTaskManagerEndpoint(
            PackageRecords.Source packages, ApplicationProcessRegistry processes) {
        this.packages = packages;
        this.processes = processes;
        attachInterface(null, "android.app.IActivityTaskManager");
    }

    private static native boolean nativeScheduleActivity(
            IBinder applicationThread,
            IBinder previousActivityToken,
            IBinder activityToken,
            Intent intent,
            ActivityInfo activityInfo,
            android.content.res.Configuration current,
            android.content.res.Configuration override);

    static native boolean nativeScheduleFinishActivity(
            IBinder applicationThread, IBinder activityToken, IBinder previousActivityToken);

    private static int transaction(String name) {
        try {
            Field field = Class.forName("android.app.IActivityTaskManager$Stub")
                    .getDeclaredField("TRANSACTION_" + name);
            field.setAccessible(true);
            return field.getInt(null);
        } catch (ReflectiveOperationException error) {
            throw new ExceptionInInitializerError(error);
        }
    }

    @SuppressWarnings("unchecked")
    private static Parcelable.Creator<Parcelable> creator(String className) {
        try {
            return (Parcelable.Creator<Parcelable>) Class.forName(className)
                    .getField("CREATOR").get(null);
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("Framework Parcelable contract changed", error);
        }
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (code != getActivityClientControllerCode && code != startActivityCode) {
            return super.onTransact(code, data, reply, flags);
        }
        data.enforceInterface("android.app.IActivityTaskManager");
        if (code == startActivityCode) {
            IBinder caller = data.readStrongBinder();
            String callingPackage = data.readString();
            data.readString(); // Calling feature ID.
            Intent intent = data.readTypedObject(Intent.CREATOR);
            data.readString(); // Resolved MIME type.
            data.readStrongBinder(); // Result-to activity token.
            data.readString(); // Result-who.
            data.readInt(); // Request code.
            data.readInt(); // Start flags.
            data.readTypedObject(creator("android.app.ProfilerInfo"));
            data.readTypedObject(Bundle.CREATOR); // ActivityOptions.
            data.enforceNoDataAvail();

            ApplicationProcessRegistry.AttachedApplication attached =
                    processes.requireCaller(Binder.getCallingPid(), caller, callingPackage);
            if (attached.uid != Binder.getCallingUid()) {
                throw new SecurityException("Activity caller UID does not match attached process");
            }
            ComponentName component = intent == null ? null : intent.getComponent();
            if (component == null) {
                reply.writeNoException();
                reply.writeInt(-91); // ActivityManager.START_INTENT_NOT_RESOLVED.
                return true;
            }
            if (!attached.packageName.equals(component.getPackageName())) {
                throw new SecurityException("Cross-package activity launch is not installed");
            }
            String record = packages.resolveInstalledPackage(attached.packageName);
            ActivityInfo info = InstalledActivityInfo.activity(
                    attached.packageName, record, component.getClassName());
            if (info == null) {
                reply.writeNoException();
                reply.writeInt(-92); // ActivityManager.START_CLASS_NOT_FOUND.
                return true;
            }
            IBinder previousToken = activityClient.topActivityToken(attached.thread);
            IBinder activityToken = new Binder();
            // The launched Activity's orientation is applied to its task first;
            // LaunchActivityItem then carries that task revision.
            android.content.res.Configuration[] configuration =
                    TaskGeometryController.launchConfiguration(attached.thread, info);
            if (!nativeScheduleActivity(attached.thread, previousToken, activityToken, intent,
                    info, configuration[0], configuration[1])) {
                throw new IllegalStateException("Activity launch transaction was rejected");
            }
            activityClient.commitLaunch(attached.thread, activityToken, info, configuration[0]);
            reply.writeNoException();
            reply.writeInt(0); // ActivityManager.START_SUCCESS.
            return true;
        }
        data.enforceNoDataAvail();
        reply.writeNoException();
        reply.writeStrongBinder(activityClient);
        return true;
    }
}
