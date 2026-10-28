package dev.aim.server;
public interface IAppHostDomainVerification {byte[] readShellState(int kind,String name,java.util.List<String> domains,int user,boolean hasUser)throws android.os.RemoteException;
 default void setShellUserSelection(String name,int user,boolean enabled,java.util.List<String> domains)throws android.os.RemoteException {throw new AssertionError();}
 default void setShellLinkHandling(String name,int user,boolean allowed)throws android.os.RemoteException {throw new AssertionError();}
}
