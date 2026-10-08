// Compile-only pinned original image type; no runtime implementation.
package android.content.pm;
abstract class BaseParceledListSlice<T> implements android.os.Parcelable {
    public BaseParceledListSlice(java.util.List<T> list) { throw new RuntimeException("stub"); }
    public java.util.List<T> getList() { throw new RuntimeException("stub"); }
    public void setInlineCountLimit(int limit) { throw new RuntimeException("stub"); }
    public void writeToParcel(android.os.Parcel parcel, int flags) { throw new RuntimeException("stub"); }
    protected abstract void writeElement(T element, android.os.Parcel parcel, int flags);
    protected abstract void writeParcelableCreator(T element, android.os.Parcel parcel);
    protected abstract android.os.Parcelable.Creator<?> readParcelableCreator(android.os.Parcel parcel, ClassLoader loader);
}
