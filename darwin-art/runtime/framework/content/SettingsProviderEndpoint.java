package dev.darwinart.runtime.content;

import android.content.AttributionSource;
import android.content.IContentProvider;
import android.content.pm.ApplicationInfo;
import android.content.pm.ProviderInfo;
import android.os.Binder;
import android.os.Bundle;
import android.os.Parcel;
import android.os.Parcelable;
import android.os.RemoteException;
import java.lang.reflect.Proxy;

/** System settings-provider Binder endpoint and its ActivityManager holder. */
public final class SettingsProviderEndpoint extends Binder {
    private static final int CALL_TRANSACTION = FIRST_CALL_TRANSACTION + 20;
    private final IContentProvider providerInterface;
    private final Parcelable holder;

    public SettingsProviderEndpoint() {
        providerInterface = (IContentProvider) Proxy.newProxyInstance(
                IContentProvider.class.getClassLoader(),
                new Class<?>[] {IContentProvider.class},
                (ignored, method, arguments) -> {
                    if ("asBinder".equals(method.getName())) return this;
                    // In-process callers (the system server) reach the
                    // provider directly, as AOSP's local Transport does.
                    if ("call".equals(method.getName()) && arguments.length == 5) {
                        return call((String) arguments[1], (String) arguments[3]);
                    }
                    throw new UnsupportedOperationException(method.getName());
                });
        attachInterface(providerInterface, "android.content.IContentProvider");
        holder = createHolder();
    }

    public Parcelable holder() { return holder; }

    private Parcelable createHolder() {
        try {
            ProviderInfo info = new ProviderInfo();
            info.authority = "settings";
            info.name = "com.android.providers.settings.SettingsProvider";
            info.packageName = "android";
            info.applicationInfo = new ApplicationInfo();
            info.applicationInfo.packageName = "android";
            info.applicationInfo.uid = android.os.Process.SYSTEM_UID;

            Class<?> type = Class.forName("android.app.ContentProviderHolder");
            Object value = type.getConstructor(ProviderInfo.class).newInstance(info);
            type.getField("provider").set(value, providerInterface);
            type.getField("noReleaseNeeded").setBoolean(value, true);
            type.getField("mLocal").setBoolean(value, false);
            return (Parcelable) value;
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("Framework ContentProviderHolder mismatch", error);
        }
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (code != CALL_TRANSACTION) return super.onTransact(code, data, reply, flags);
        data.enforceInterface("android.content.IContentProvider");
        AttributionSource.CREATOR.createFromParcel(data);
        String authority = data.readString();
        data.readString(); // Settings provider method: GET_secure/GET_global/etc.
        String key = data.readString();
        data.readBundle();
        data.enforceNoDataAvail();
        Bundle result = call(authority, key);
        reply.writeNoException();
        reply.writeBundle(result);
        return true;
    }

    private static Bundle call(String authority, String key) {
        if (!"settings".equals(authority)) throw new IllegalArgumentException("Unknown authority");
        Bundle result = new Bundle();
        if ("android_id".equals(key)) result.putString("value", stableAndroidId());
        return result;
    }

    private static String stableAndroidId() {
        String configured = System.getenv("DARWIN_ART_ANDROID_ID");
        return configured == null || configured.isEmpty()
                ? "darwinart00000001" : configured;
    }
}
