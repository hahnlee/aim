package android.content.pm;

import java.util.ArrayList;
import java.util.List;

/** Test-only stand-in for the hidden Android 16 ParceledListSlice. */
public class ParceledListSlice<T> {
    private final List<T> list;

    public ParceledListSlice(List<T> list) { this.list = new ArrayList<>(list); }

    public List<T> getList() { return list; }
}
