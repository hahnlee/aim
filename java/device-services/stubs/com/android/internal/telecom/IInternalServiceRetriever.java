// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.internal.telecom;

import android.os.IBinder;

public interface IInternalServiceRetriever extends android.os.IInterface {
    abstract class Stub extends android.os.Binder implements IInternalServiceRetriever {
        public static IInternalServiceRetriever asInterface(IBinder obj) { throw new RuntimeException("stub"); }
    }
}
