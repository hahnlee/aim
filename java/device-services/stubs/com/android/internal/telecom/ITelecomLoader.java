// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.internal.telecom;

import android.os.IBinder;

public interface ITelecomLoader extends android.os.IInterface {
    ITelecomService createTelecomService(IInternalServiceRetriever retriever, String sysUiName)
            throws android.os.RemoteException;

    abstract class Stub extends android.os.Binder implements ITelecomLoader {
        public static ITelecomLoader asInterface(IBinder obj) { throw new RuntimeException("stub"); }
    }
}
