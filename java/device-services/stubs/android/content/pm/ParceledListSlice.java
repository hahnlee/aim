// Compile-only pinned original image type; no runtime implementation.
package android.content.pm;
public class ParceledListSlice<T extends android.os.Parcelable> extends BaseParceledListSlice<T> {
    @SuppressWarnings("rawtypes")
    public static final android.os.Parcelable.ClassLoaderCreator<ParceledListSlice> CREATOR = null;
    public ParceledListSlice(java.util.List<T> list) { super(list); throw new RuntimeException("stub"); }
    public java.util.List<T> getList() { throw new RuntimeException("stub"); }
    public void setInlineCountLimit(int limit) { throw new RuntimeException("stub"); }
    public void writeToParcel(android.os.Parcel parcel, int flags) { throw new RuntimeException("stub"); }
    public int describeContents() { throw new RuntimeException("stub"); }
    protected void writeElement(T element, android.os.Parcel parcel, int flags) { throw new RuntimeException("stub"); }
    protected void writeParcelableCreator(T element, android.os.Parcel parcel) { throw new RuntimeException("stub"); }
    protected android.os.Parcelable.Creator<?> readParcelableCreator(android.os.Parcel parcel, ClassLoader loader) { throw new RuntimeException("stub"); }
}
