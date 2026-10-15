// Compile-only original Binder interface.
package android.content;
public interface IIntentSender extends android.os.IInterface {
    void send(int code,Intent intent,String type,android.os.IBinder whitelist,IIntentReceiver finished,String permission,android.os.Bundle options) throws android.os.RemoteException;
    abstract class Stub extends android.os.Binder implements IIntentSender {
        public Stub(){throw new RuntimeException("stub");}
        public android.os.IBinder asBinder(){throw new RuntimeException("stub");}
    }
}
