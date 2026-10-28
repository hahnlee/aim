package com.android.server.pm;
import android.content.Context;
import android.os.Binder;
import android.os.Process;
import com.android.server.pm.dex.DexManager;
import com.android.server.pm.dex.DynamicCodeLogger;
import dev.aim.server.IPackageShutdownBridge;
/** Independent original helpers; no PackageManagerService reference or delegation. */
public final class NativePackageShutdownBridge extends IPackageShutdownBridge.Stub {
    private final Context context;private final Runnable compiler;private final DexManager dex;private final DynamicCodeLogger dynamic;
    public NativePackageShutdownBridge(Context context,CompilerStats compiler,DexManager dex,DynamicCodeLogger dynamic){
        this(context, java.util.Objects.requireNonNull(compiler)::writeNow, dex, dynamic);
    }
    /** The app host retains its native CompilerStats; the remaining owners stay original. */
    public NativePackageShutdownBridge(Context context,Runnable compiler,DexManager dex,DynamicCodeLogger dynamic){
        this.context=java.util.Objects.requireNonNull(context);this.compiler=java.util.Objects.requireNonNull(compiler);
        this.dex=java.util.Objects.requireNonNull(dex);this.dynamic=java.util.Objects.requireNonNull(dynamic);
    }
    @Override public void writeStatisticsNow(){
        if(Binder.getCallingUid()!=Process.SYSTEM_UID)throw new SecurityException("native shutdown owner required");
        compiler.run();dex.writePackageDexUsageNow();dynamic.writeNow();
        if(!android.crashrecovery.flags.Flags.refactorCrashrecovery())
            com.android.server.PackageWatchdog.getInstance(context).writeNow();
    }
}
