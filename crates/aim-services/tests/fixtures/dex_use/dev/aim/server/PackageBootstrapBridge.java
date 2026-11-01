package dev.aim.server;
public final class PackageBootstrapBridge {
 public static int captures;
 public static PackageLocal captureLocal(IDexUsePackages backend) throws android.os.RemoteException,java.io.IOException {
  if(backend.getSnapshot()==null)throw new java.io.IOException("missing capture");
  captures++;
  return new PackageLocal();
 }
}
