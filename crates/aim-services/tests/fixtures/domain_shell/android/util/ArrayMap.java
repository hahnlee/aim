package android.util;
public class ArrayMap<K,V> extends java.util.TreeMap<K,V> {
 public ArrayMap(){super(java.util.Comparator.comparingInt(key -> key == null ? 0 : key.hashCode()));}
 public K keyAt(int i){return new java.util.ArrayList<>(keySet()).get(i);} public V valueAt(int i){return get(keyAt(i));}
 public V removeAt(int i){return remove(keyAt(i));}
}
