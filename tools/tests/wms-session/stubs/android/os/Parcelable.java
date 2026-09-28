package android.os;

public interface Parcelable {
    int PARCELABLE_WRITE_RETURN_VALUE = 1;
    interface Creator<T> {
        T createFromParcel(Parcel source);
        T[] newArray(int size);
    }

    void writeToParcel(Parcel dest, int flags);
    int describeContents();
}
