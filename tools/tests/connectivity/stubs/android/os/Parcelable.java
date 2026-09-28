package android.os;

/** Test-only Parcelable surface used by the AIDL query contract. */
public interface Parcelable {
    interface Creator<T> {
        T createFromParcel(Parcel source);
        T[] newArray(int size);
    }

    void writeToParcel(Parcel dest, int flags);
}
