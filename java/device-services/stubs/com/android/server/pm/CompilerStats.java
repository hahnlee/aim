// Compile-only pinned image API.
package com.android.server.pm;
public class CompilerStats extends AbstractStatsBase<Void> { void writeNow(){throw new RuntimeException("stub");}  public CompilerStats(){super("unused compile stub","unused compile stub",false);throw new RuntimeException("stub");}
 public PackageStats getOrCreatePackageStats(String name){throw new RuntimeException("stub");}
 boolean maybeWriteAsync(){throw new RuntimeException("stub");}
 static class PackageStats {public PackageStats(String name){throw new RuntimeException("stub");}public void setCompileTime(String path,long milliseconds){throw new RuntimeException("stub");}}
}
