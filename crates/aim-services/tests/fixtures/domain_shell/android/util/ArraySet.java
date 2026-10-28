package android.util;
public class ArraySet<E> extends java.util.TreeSet<E> {
 public ArraySet(){super(java.util.Comparator.comparingInt(Object::hashCode));}
 public ArraySet(java.util.Collection<E> c){this();addAll(c);} public E valueAt(int i){return new java.util.ArrayList<>(this).get(i);}
}
