package dev.aim.probe;

import android.content.ContentResolver;
import android.content.Context;
import android.content.IContentProvider;
import android.content.ContentProvider;
import android.content.pm.ProviderInfo;
import android.database.MatrixCursor;
import android.os.Bundle;
import android.provider.CalendarContract;

import java.lang.reflect.Method;
import java.lang.reflect.Proxy;

final class ProbeContentResolver extends ContentResolver {
    private final IContentProvider settingsProvider;
    private final IContentProvider calendarProvider;
    private final IContentProvider hostDocumentProvider;
    private final IContentProvider mediaStoreProvider;

    ProbeContentResolver(Context context) {
        super(context);
        settingsProvider = (IContentProvider) Proxy.newProxyInstance(
                ProbeContentResolver.class.getClassLoader(),
                new Class<?>[] { IContentProvider.class },
                (proxy, method, args) -> {
                    if (method.getName().equals("call")) {
                        Bundle result = new Bundle();
                        // An absent Android settings row is represented by a
                        // Bundle with no value. Returning a universal "1"
                        // corrupts typed settings such as SQLite's comma-
                        // separated compatibility flags.
                        return result;
                    }
                    if (method.getName().equals("query")) {
                        String[] projection = null;
                        if (args != null) {
                            for (Object arg : args) {
                                if (arg instanceof String[]) {
                                    projection = (String[]) arg;
                                }
                            }
                        }
                        // A provider with no persisted rows returns an empty
                        // cursor, never null. Framework loaders and unchanged
                        // APKs rely on the cursor contract even when the host
                        // has no Android alarm/settings database yet.
                        return new MatrixCursor(
                                projection == null ? new String[0] : projection);
                    }
                    Class<?> returnType = method.getReturnType();
                    if (returnType == boolean.class) return false;
                    if (returnType == int.class) return 0;
                    if (returnType == long.class) return 0L;
                    return null;
                });
        calendarProvider = attachCalendarProvider(context);
        hostDocumentProvider = attachProvider(
                context, new ProbeHostDocumentProvider(),
                ProbeHostDocumentProvider.AUTHORITY);
        mediaStoreProvider = attachProvider(
                context, new ProbeMediaStoreProvider(),
                ProbeMediaStoreProvider.AUTHORITY);
    }

    private static IContentProvider attachCalendarProvider(Context context) {
        return attachProvider(context, new ProbeCalendarProvider(),
                CalendarContract.AUTHORITY);
    }

    private static IContentProvider attachProvider(
            Context context, ContentProvider provider, String authority) {
        try {
            ProviderInfo info = new ProviderInfo();
            info.authority = authority;
            info.exported = true;
            provider.attachInfo(context, info);
            Method transport = ContentProvider.class.getDeclaredMethod("getIContentProvider");
            transport.setAccessible(true);
            return (IContentProvider) transport.invoke(provider);
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("Could not attach Calendar provider", error);
        }
    }

    protected IContentProvider acquireProvider(Context context, String name) {
        if (CalendarContract.AUTHORITY.equals(name)) return calendarProvider;
        if (ProbeHostDocumentProvider.AUTHORITY.equals(name)) {
            return hostDocumentProvider;
        }
        if (ProbeMediaStoreProvider.AUTHORITY.equals(name)) {
            return mediaStoreProvider;
        }
        return settingsProvider;
    }

    protected IContentProvider acquireUnstableProvider(Context context, String name) {
        if (CalendarContract.AUTHORITY.equals(name)) return calendarProvider;
        if (ProbeHostDocumentProvider.AUTHORITY.equals(name)) {
            return hostDocumentProvider;
        }
        if (ProbeMediaStoreProvider.AUTHORITY.equals(name)) {
            return mediaStoreProvider;
        }
        return settingsProvider;
    }

    public boolean releaseProvider(IContentProvider provider) {
        return true;
    }

    public boolean releaseUnstableProvider(IContentProvider provider) {
        return true;
    }

    public void unstableProviderDied(IContentProvider provider) {}
}
