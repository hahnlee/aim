package android.app.servertransaction;

import android.app.IApplicationThread;
import android.os.RemoteException;

/** Compile-only Android 16 signature stub; runtime resolution uses framework.jar. */
public class ClientTransaction {
    public ClientTransaction(IApplicationThread client) {}
    public void addTransactionItem(ClientTransactionItem item) {}
    /** Returns the scheduling failure instead of throwing it. */
    public RemoteException schedule() { return null; }
}
