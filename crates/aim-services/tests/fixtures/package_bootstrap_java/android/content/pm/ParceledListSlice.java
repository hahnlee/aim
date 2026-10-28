package android.content.pm;
public class ParceledListSlice<T extends android.os.Parcelable> {
    private final java.util.List<T> list;
    public ParceledListSlice(java.util.List<T> list) { this.list = list; }
    public java.util.List<T> getList() { return list; }
}
