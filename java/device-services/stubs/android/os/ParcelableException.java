// Compile-only API of the pinned original class; never included in guest output.
package android.os;
public final class ParcelableException extends RuntimeException implements Parcelable {
    public ParcelableException(Throwable cause) {super(cause);}
    public int describeContents() {return 0;}
    public void writeToParcel(Parcel parcel,int flags) {throw new RuntimeException("stub");}
}
