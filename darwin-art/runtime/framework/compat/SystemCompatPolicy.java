package dev.darwinart.runtime.compat;

import android.content.Context;
import android.content.pm.ApplicationInfo;
import android.os.Environment;
import com.android.internal.compat.AndroidBuildClassifier;
import com.android.server.compat.CompatChange;
import com.android.server.compat.CompatConfig;
import com.android.server.compat.config.Config;
import java.io.File;

/** System-owned read-only access to the pinned original AOSP policy evaluator. */
public final class SystemCompatPolicy {
    private static volatile CompatConfig policy;
    private static boolean initializationStarted;
    private SystemCompatPolicy() {}

    public static synchronized void initialize(Context context) throws Exception {
        if (context == null) throw new NullPointerException("system Context");
        if (initializationStarted) throw new IllegalStateException("Compatibility policy already initialized");
        initializationStarted = true;
        CompatConfig loaded = new CompatConfig(new AndroidBuildClassifier(), context);
        int definitions = 0;
        for (File file : SystemCompatCatalog.files(Environment.getRootDirectory(), new File("/apex"))) {
            Config config = SystemCompatConfigReader.read(file);
            // R8 erased getCompatChange()'s List<Change> signature.
            for (Object change : config.getCompatChange()) {
                loaded.addChange(new CompatChange((com.android.server.compat.config.Change) change));
                definitions++;
            }
        }
        if (definitions == 0) throw new IllegalStateException("Empty platform compatibility catalog");
        policy = loaded;
    }

    public static long[] disabledChanges(ApplicationInfo application) {
        if (application == null) throw new NullPointerException("ApplicationInfo");
        return current().getDisabledChanges(application);
    }

    /** PlatformCompat.isChangeEnabled for a package being parsed or queried. */
    public static boolean isChangeEnabled(long changeId, ApplicationInfo application) {
        if (application == null) throw new NullPointerException("ApplicationInfo");
        return current().isChangeEnabled(changeId, application);
    }

    public static long[] loggableChanges(ApplicationInfo application) {
        if (application == null) throw new NullPointerException("ApplicationInfo");
        return current().getLoggableChanges(application);
    }

    private static CompatConfig current() {
        CompatConfig value = policy;
        if (value == null) throw new IllegalStateException("Compatibility policy not initialized");
        return value;
    }
}
