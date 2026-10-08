// Compile-only pinned image API; never packaged as runtime implementation.
package com.android.server.art.model;
public abstract class DexoptResult {
 protected DexoptResult(){throw new RuntimeException("stub");}
 public static final int DEXOPT_SKIPPED=10,DEXOPT_PERFORMED=20,DEXOPT_FAILED=30,DEXOPT_CANCELLED=40;
 public int getFinalStatus(){throw new RuntimeException("stub");}
 public abstract String getReason();
 public abstract java.util.List<PackageDexoptResult> getPackageDexoptResults();
 public static abstract class PackageDexoptResult {
  protected PackageDexoptResult(){throw new RuntimeException("stub");}
  public abstract String getPackageName();
  public int getStatus(){throw new RuntimeException("stub");}
  public boolean hasUpdatedArtifacts(){throw new RuntimeException("stub");}
  public abstract java.util.List<DexContainerFileDexoptResult> getDexContainerFileDexoptResults();
 }
 public static abstract class DexContainerFileDexoptResult {
  protected DexContainerFileDexoptResult(){throw new RuntimeException("stub");}
  public abstract String getDexContainerFile();
  public abstract long getDex2oatWallTimeMillis();
  public abstract long getSizeBytes();
  public abstract long getSizeBeforeBytes();
 }
}
