// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content;

import android.database.IContentObserver;
import android.net.Uri;
import android.os.RemoteException;

public interface IContentService extends android.os.IInterface {
    void registerContentObserver(Uri uri, boolean notifyForDescendants,
            IContentObserver observer, int userHandle, int targetSdkVersion)
            throws RemoteException;
}
