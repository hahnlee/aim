package android.view;

import android.os.IBinder;
import android.os.Parcel;
import android.os.Parcelable;

/** Compile signatures only. Never packaged; original framework owns endpoint transport. */
public final class InputChannel implements Parcelable {
    // Existing shared fixture compilation forces this checked exception on the
    // WMS caller. It is not part of the original Android source API or JVM ABI.
    // Remove together with that caller's catch when the fixture API is isolated.
    public static native InputChannel[] openInputChannelPair(String name) throws java.io.IOException;
    public native String getName();
    public native IBinder getToken();
    public native void dispose();
    public native int describeContents();
    public native void writeToParcel(Parcel out, int flags);
}
