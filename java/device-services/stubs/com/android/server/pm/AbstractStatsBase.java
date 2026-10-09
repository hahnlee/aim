// Compile-only image ABI, not included at runtime.
package com.android.server.pm;public abstract class AbstractStatsBase<T> {
 public AbstractStatsBase(String file,String thread,boolean lock){throw new RuntimeException("stub");}
 public android.util.AtomicFile getFile(){throw new RuntimeException("stub");}
}
