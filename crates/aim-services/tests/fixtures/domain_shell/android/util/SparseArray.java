package android.util;
public class SparseArray<E> {private final java.util.TreeMap<Integer,E> m=new java.util.TreeMap<>();
 public void put(int k,E v){m.put(k,v);} public E get(int k){return m.get(k);} public int size(){return m.size();}
 public int keyAt(int i){return new java.util.ArrayList<>(m.keySet()).get(i);} public E valueAt(int i){return m.get(keyAt(i));}
}
