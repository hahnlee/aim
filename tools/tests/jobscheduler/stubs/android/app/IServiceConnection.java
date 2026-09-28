package android.app;

import android.content.ComponentName;
import android.os.IBinder;
import android.os.RemoteException;

public interface IServiceConnection {
    void connected(ComponentName name, IBinder service, boolean dead) throws RemoteException;
    IBinder asBinder();
}
