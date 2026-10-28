// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content.pm.verify.domain;

public interface IDomainVerificationManager extends android.os.IInterface {
    java.util.List<String> queryValidVerificationPackageNames() throws android.os.RemoteException;
    DomainVerificationInfo getDomainVerificationInfo(String name) throws android.os.RemoteException;
    int setDomainVerificationStatus(String id, DomainSet domains, int state) throws android.os.RemoteException;
    abstract class Stub extends android.os.Binder implements IDomainVerificationManager {
        @Override public android.os.IBinder asBinder() { throw new RuntimeException("stub"); }
        public static IDomainVerificationManager asInterface(android.os.IBinder binder) { throw new RuntimeException("stub"); }
    }
}
