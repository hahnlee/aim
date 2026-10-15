package android.content.pm;
import android.os.ParcelFileDescriptor;
interface IPackageInstallerSessionFileSystemConnector { void writeData(String name,long offsetBytes,long lengthBytes,in ParcelFileDescriptor incomingFd); }
