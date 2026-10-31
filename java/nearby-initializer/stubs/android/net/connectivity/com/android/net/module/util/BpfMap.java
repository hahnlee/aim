// Compile-only android-16.0.0_r1 API; checked against the pinned image.
package android.net.connectivity.com.android.net.module.util;
public class BpfMap<K extends Struct,V extends Struct> implements IBpfMap<K,V> {
public static final int BPF_F_RDONLY=8;
 public BpfMap(String path,int flags,Class<K> k,Class<V> v) throws android.system.ErrnoException{throw new RuntimeException("stub");}
 public V getValue(K k) throws android.system.ErrnoException{throw new RuntimeException("stub");}
}
