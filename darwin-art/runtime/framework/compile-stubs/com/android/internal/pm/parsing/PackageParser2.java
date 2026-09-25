package com.android.internal.pm.parsing;

import android.content.pm.ApplicationInfo;
import android.util.DisplayMetrics;
import com.android.internal.pm.parsing.pkg.ParsedPackage;
import com.android.internal.pm.pkg.parsing.ParsingPackageUtils;
import java.io.File;

/** Compile-only Android 16 signature stub; runtime resolution uses framework.jar/services.jar. */
public class PackageParser2 implements AutoCloseable {
    public PackageParser2(String[] separateProcesses, DisplayMetrics displayMetrics,
            IPackageCacher cacher, Callback callback) {
        throw new RuntimeException("stub");
    }
    /** Throws PackageParser.PackageParserException at runtime. */
    public ParsedPackage parsePackage(File packageFile, int flags, boolean useCaches)
            throws Exception {
        throw new RuntimeException("stub");
    }
    @Override public void close() { throw new RuntimeException("stub"); }

    public abstract static class Callback implements ParsingPackageUtils.Callback {
        public abstract boolean isChangeEnabled(long changeId, ApplicationInfo appInfo);
    }
}
