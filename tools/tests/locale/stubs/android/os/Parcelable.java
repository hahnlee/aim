package android.os;

/** Test-only marker for the typed Parcelable operations used by LocaleList. */
public interface Parcelable {
    int PARCELABLE_WRITE_RETURN_VALUE = 1;

    interface Creator<T> {
        T createFromParcel(Parcel source);

        T[] newArray(int size);
    }
}
