// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.os;

import java.io.FileDescriptor;

public class ParcelFileDescriptor implements Parcelable {
    public static final Parcelable.Creator<ParcelFileDescriptor> CREATOR = null;

    public ParcelFileDescriptor(FileDescriptor fd) { throw new RuntimeException("stub"); }
    @Override
    public void writeToParcel(Parcel out, int flags) { throw new RuntimeException("stub"); }
}
