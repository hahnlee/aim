package dev.darwinart.runtime.pm;

import android.content.res.AssetManager;
import android.content.res.Resources;
import android.os.Bundle;
import android.util.TypedValue;
import java.lang.reflect.Array;
import java.lang.reflect.Constructor;
import java.lang.reflect.Field;
import java.lang.reflect.Method;

/** Resolves manifest android:value references with the installed package's resources. */
final class InstalledResourceValue {
    private InstalledResourceValue() {}

    static void put(Bundle bundle, String name, InstalledPackageRecord installed, int resourceId) {
        AssetManager assets = createAssetManager(installed);
        try {
            Resources system = Resources.getSystem();
            Resources resources = new Resources(
                    assets, system.getDisplayMetrics(), system.getConfiguration());
            TypedValue value = new TypedValue();
            resources.getValue(resourceId, value, true);
            putTypedValue(bundle, name, value);
        } finally {
            assets.close();
        }
    }

    private static AssetManager createAssetManager(InstalledPackageRecord installed) {
        try {
            Class<?> apkAssetsClass = Class.forName("android.content.res.ApkAssets");
            Method loadFromPath = apkAssetsClass.getDeclaredMethod("loadFromPath", String.class);
            loadFromPath.setAccessible(true);
            String[] paths = installed.splitPaths();
            Object configured = Array.newInstance(apkAssetsClass, paths.length + 1);
            Array.set(configured, 0, loadFromPath.invoke(null, installed.baseApk));
            for (int index = 0; index < paths.length; index++) {
                Array.set(configured, index + 1, loadFromPath.invoke(null, paths[index]));
            }

            Constructor<AssetManager> constructor =
                    AssetManager.class.getDeclaredConstructor(boolean.class);
            constructor.setAccessible(true);
            AssetManager assets = constructor.newInstance(true);
            Field objectField = AssetManager.class.getDeclaredField("mObject");
            objectField.setAccessible(true);
            Method setApkAssets = AssetManager.class.getDeclaredMethod(
                    "nativeSetApkAssets", long.class, configured.getClass(),
                    boolean.class, boolean.class);
            setApkAssets.setAccessible(true);
            setApkAssets.invoke(null, objectField.getLong(assets), configured, true, false);
            Field assetsField = AssetManager.class.getDeclaredField("mApkAssets");
            assetsField.setAccessible(true);
            assetsField.set(assets, configured);
            return assets;
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("installed resources are unavailable", error);
        }
    }

    private static void putTypedValue(Bundle bundle, String name, TypedValue value) {
        if (value.type == TypedValue.TYPE_STRING) {
            bundle.putString(name, value.string == null ? null : value.string.toString());
        } else if (value.type == TypedValue.TYPE_FLOAT) {
            bundle.putFloat(name, value.getFloat());
        } else if (value.type == TypedValue.TYPE_INT_BOOLEAN) {
            bundle.putBoolean(name, value.data != 0);
        } else if (value.type >= TypedValue.TYPE_FIRST_INT
                && value.type <= TypedValue.TYPE_LAST_INT) {
            bundle.putInt(name, value.data);
        } else {
            throw new IllegalArgumentException("unsupported manifest metadata resource type");
        }
    }
}
