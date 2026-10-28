package android.util;
public class ArraySet<E> extends java.util.LinkedHashSet<E> {
    public E valueAt(int index) { return new java.util.ArrayList<>(this).get(index); }
}
