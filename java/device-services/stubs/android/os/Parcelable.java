// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.os;

public interface Parcelable {
    int PARCELABLE_WRITE_RETURN_VALUE = 0x0001;
    void writeToParcel(Parcel dest, int flags);

    interface Creator<T> {
        T createFromParcel(Parcel in);
    }
}
