package dev.aim.server;
import android.os.IBinder;
interface ISystemProcess {
    IBinder getInternalServiceRepository();
    void attachDomainVerification(IBinder frontend);
    void attachSystemProcess(IBinder thread, IForegroundServicePolicy foreground);
    abstract class Stub {
        public static ISystemProcess asInterface(IBinder binder) {
            return new ISystemProcess() {
                public IBinder getInternalServiceRepository() { throw new AssertionError(); }
                public void attachDomainVerification(IBinder frontend) {
                    if (frontend == null) throw new AssertionError("missing domain front end");
                    SystemProcessHostTest.events.add("attachDomain");
                }
                public void attachSystemProcess(IBinder thread, IForegroundServicePolicy policy) {
                    if (thread != SystemProcessHostTest.applicationThread
                            || policy != SystemProcessHostTest.foregroundPolicy) {
                        throw new AssertionError("lost application thread or foreground policy");
                    }
                    SystemProcessHostTest.events.add("attachSystem");
                }
            };
        }
    }
}
