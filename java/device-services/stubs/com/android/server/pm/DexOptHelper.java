// Compile-only pinned image API; never packaged as runtime implementation.
package com.android.server.pm;
public class DexOptHelper {
 public static com.android.server.art.ArtManagerLocal getArtManagerLocal(){throw new RuntimeException("stub");}
 public static com.android.server.art.DexUseManagerLocal getDexUseManagerLocal(){throw new RuntimeException("stub");}
 public static boolean artManagerLocalIsInitialized(){throw new RuntimeException("stub");}
 public DexOptHelper(PackageManagerService service){throw new RuntimeException("stub");}
}
