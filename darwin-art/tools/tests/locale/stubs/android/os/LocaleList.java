package android.os;

/** Test-only empty LocaleList representation for the Binder wire test. */
public final class LocaleList implements Parcelable {
    public static final Creator<LocaleList> CREATOR = new Creator<LocaleList>() {
        @Override
        public LocaleList createFromParcel(Parcel source) {
            return (LocaleList) source.readValue();
        }

        @Override
        public LocaleList[] newArray(int size) {
            return new LocaleList[size];
        }
    };

    private static final LocaleList EMPTY = new LocaleList();

    private LocaleList() {}

    public static LocaleList getEmptyLocaleList() {
        return EMPTY;
    }

    public boolean isEmpty() {
        return true;
    }
}
