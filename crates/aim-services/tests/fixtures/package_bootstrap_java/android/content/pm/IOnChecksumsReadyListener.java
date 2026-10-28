package android.content.pm;
public interface IOnChecksumsReadyListener {
    void onChecksumsReady(java.util.List<ApkChecksum> sums) throws android.os.RemoteException;
    abstract class Stub implements IOnChecksumsReadyListener {}
}
