package dev.aim.server;
import android.content.Context;
import android.os.Binder;

/** Original resources/flags and actual inode creation, independent of PMS. */
final class PackageBootConfigurationLeaf extends IPackageBootConfigurationLeaf.Stub {
    private final Context context;
    private final boolean factoryTest;
    PackageBootConfigurationLeaf(Context context, boolean factoryTest) {
        this.context = context; this.factoryTest = factoryTest;
    }
    private void enforce() {
        if (Binder.getCallingUid() != 1000) throw new SecurityException("Native boot configuration requires system uid");
    }
    @Override public int getDensity() { enforce(); return context.getResources().getDisplayMetrics().densityDpi; }
    @Override public int[] getUserIds() {
        enforce();
        var users = com.android.server.LocalServices.getService(com.android.server.pm.UserManagerInternal.class);
        if (users == null) throw new IllegalStateException("Original UM boot inventory owner absent");
        // PackageManagerService constructor2195: exclude partial, retain dying
        // and pre-created users. Do not infer a system-user-only first boot.
        var records = users.getUsers(true, false, false);
        if (records == null || records.isEmpty()) throw new IllegalStateException("Original UM boot inventory empty");
        int[] ids = new int[records.size()];
        for (int index = 0; index < ids.length; index++) {
            var user = records.get(index);
            if (user == null || user.id < 0) throw new IllegalStateException("Invalid original UM boot user");
            ids[index] = user.id;
        }
        if (users != com.android.server.LocalServices.getService(com.android.server.pm.UserManagerInternal.class))
            throw new IllegalStateException("Original UM boot inventory owner replaced");
        return ids;
    }
    @Override public boolean isFactoryTest() { enforce(); return factoryTest; }
    @Override public boolean isDependencyInstallerEnabled() { enforce(); return com.android.internal.hidden_from_bootclasspath.android.content.pm.Flags.sdkDependencyInstaller(); }
    @Override public boolean fixSystemAppsFirstInstallTime() { enforce(); return com.android.internal.hidden_from_bootclasspath.android.content.pm.Flags.fixSystemAppsFirstInstallTime(); }
    @Override public byte[] creationInode(String path) {
        enforce();
        try { return com.android.server.pm.NativeBootInodeInputs.capture(path); }
        catch (java.io.IOException | android.system.ErrnoException error) { throw new IllegalStateException("Native boot inode creation failed", error); }
    }
    private static int[] pack(String value, char base) {
        if (value.isEmpty()) return new int[]{0, 0};
        if (value.length() == 2) return new int[]{value.charAt(0), value.charAt(1)};
        if (value.length() != 3) throw new IllegalStateException("Locale code cannot be represented by ResTable_config");
        int first = value.charAt(0) - base;
        int second = value.charAt(1) - base;
        int third = value.charAt(2) - base;
        if (first < 0 || first > 31 || second < 0 || second > 31 || third < 0 || third > 31)
            throw new IllegalStateException("Locale packed code invalid");
        return new int[]{0x80 | (third << 2) | (second >> 3), ((second << 5) | first) & 0xff};
    }
    @Override public byte[] captureResources() {
        enforce();
        var resources = context.getResources();
        var c = resources.getConfiguration();
        var metrics = resources.getDisplayMetrics();
        if (c.getLocales().isEmpty()) throw new IllegalStateException("Actual resource locale list empty");
        var locale = c.getLocales().get(0);
        int[] language = pack(locale.getLanguage(), 'a');
        int[] country = pack(locale.getCountry(), '0');
        int hidden = c.keyboardHidden == 1 && c.hardKeyboardHidden == 2 ? 3 : c.keyboardHidden;
        // ResourcesImpl.updateConfiguration -> AssetManager.NativeSetConfiguration:
        // larger physical dimension first; minorVersion remains native memset(0).
        int[] values = {c.mcc, c.mnc, language[0], language[1], country[0], country[1],
                c.orientation, c.touchscreen, c.densityDpi, c.keyboard, c.navigation,
                hidden, c.getGrammaticalGender(), Math.max(metrics.widthPixels, metrics.heightPixels),
                Math.min(metrics.widthPixels, metrics.heightPixels), android.os.Build.VERSION.RESOURCES_SDK_INT,
                0, c.screenLayout, c.uiMode, c.smallestScreenWidthDp, c.screenWidthDp,
                c.screenHeightDp, (c.screenLayout & 0x300) >> 8, c.colorMode};
        android.os.Parcel out = android.os.Parcel.obtain();
        try { out.writeInt(1); out.writeInt(values.length); for (int value : values) out.writeInt(value); return out.marshall(); }
        finally { out.recycle(); }
    }
    @Override public byte[] getLiveProperties(String[] keys) {
        enforce();
        if (keys == null || keys.length == 0) throw new IllegalArgumentException("Actual property inventory absent");
        android.os.Parcel out = android.os.Parcel.obtain();
        try {
            out.writeInt(1); out.writeInt(keys.length);
            java.util.HashSet<String> seen = new java.util.HashSet<>();
            for (String key : keys) {
                if (key == null || key.isEmpty() || !seen.add(key)) throw new IllegalArgumentException("Invalid actual property key");
                var handle = android.os.SystemProperties.find(key);
                if (handle == null) throw new IllegalStateException("Actual required property absent: " + key);
                out.writeString(key); out.writeString(handle.get());
            }
            return out.marshall();
        } finally { out.recycle(); }
    }
    @Override public String getParserCacheDirectory() {
        enforce();
        java.io.File directory = com.android.server.pm.PackageManagerServiceUtils.preparePackageParserCache(
                android.os.Build.IS_ENG, android.os.Build.IS_USERDEBUG, android.os.Build.VERSION.INCREMENTAL);
        return directory == null ? null : directory.getPath();
    }
}
