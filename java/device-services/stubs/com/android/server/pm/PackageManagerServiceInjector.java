// Compile-only image ABI, not included at runtime.
package com.android.server.pm;public class PackageManagerServiceInjector {
    public PackageManagerServiceInjector(
            android.content.Context arg0,
            com.android.server.pm.PackageManagerTracedLock arg1,
            com.android.server.pm.Installer arg2,
            com.android.server.pm.PackageManagerTracedLock arg3,
            com.android.server.pm.PackageAbiHelper arg4,
            android.os.Handler arg5,
            java.util.List arg6,
            com.android.server.pm.PackageManagerServiceInjector.Producer arg7,
            com.android.server.pm.PackageManagerServiceInjector.Producer arg8,
            com.android.server.pm.PackageManagerServiceInjector.Producer arg9,
            com.android.server.pm.PackageManagerServiceInjector.Producer arg10,
            com.android.server.pm.PackageManagerServiceInjector.Producer arg11,
            com.android.server.pm.PackageManagerServiceInjector.Producer arg12,
            com.android.server.pm.PackageManagerServiceInjector.Producer arg13,
            com.android.server.pm.PackageManagerServiceInjector.Producer arg14,
            com.android.server.pm.PackageManagerServiceInjector.Producer arg15,
            com.android.server.pm.PackageManagerServiceInjector.Producer arg16,
            com.android.server.pm.PackageManagerServiceInjector.Producer arg17,
            com.android.server.pm.PackageManagerServiceInjector.Producer arg18,
            com.android.server.pm.PackageManagerServiceInjector.Producer arg19,
            com.android.server.pm.PackageManagerServiceInjector.Producer arg20,
            com.android.server.pm.PackageManagerServiceInjector.Producer arg21,
            com.android.server.pm.PackageManagerServiceInjector.Producer arg22,
            com.android.server.pm.PackageManagerServiceInjector.Producer arg23,
            com.android.server.pm.PackageManagerServiceInjector.Producer arg24,
            com.android.server.pm.PackageManagerServiceInjector.Producer arg25,
            com.android.server.pm.PackageManagerServiceInjector.Producer arg26,
            com.android.server.pm.PackageManagerServiceInjector.ProducerWithArgument arg27,
            com.android.server.pm.PackageManagerServiceInjector.Producer arg28,
            com.android.server.pm.PackageManagerServiceInjector.Producer arg29,
            com.android.server.pm.PackageManagerServiceInjector.Producer arg30,
            com.android.server.pm.PackageManagerServiceInjector.Producer arg31,
            com.android.server.pm.PackageManagerServiceInjector.SystemWrapper arg32,
            com.android.server.pm.PackageManagerServiceInjector.ServiceProducer arg33,
            com.android.server.pm.PackageManagerServiceInjector.ServiceProducer arg34,
            com.android.server.pm.PackageManagerServiceInjector.Producer arg35,
            com.android.server.pm.PackageManagerServiceInjector.Producer arg36,
            com.android.server.pm.PackageManagerServiceInjector.Producer arg37,
            com.android.server.pm.PackageManagerServiceInjector.Producer arg38,
            com.android.server.pm.PackageManagerServiceInjector.Producer arg39) { throw new RuntimeException("stub"); }
    interface Producer<T> { T produce(PackageManagerServiceInjector injector, PackageManagerService service); }
    public interface ProducerWithArgument<T, A> { T produce(PackageManagerServiceInjector injector, PackageManagerService service, A argument); }
    public interface ServiceProducer { Object produce(Class<?> type); }
    public interface SystemWrapper { void disablePackageCaches(); void enablePackageCaches(); }
}
