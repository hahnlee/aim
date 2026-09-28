package android.os;

/** Test-only empty Parcelable Bundle used for the no-restrictions response. */
public final class Bundle implements Parcelable {
    @Override
    public void writeToParcel(Parcel dest, int flags) {}

    public static final Parcelable.Creator<Bundle> CREATOR =
            new Parcelable.Creator<Bundle>() {
                @Override
                public Bundle createFromParcel(Parcel source) {
                    return new Bundle();
                }

                @Override
                public Bundle[] newArray(int size) {
                    return new Bundle[size];
                }
            };
}
