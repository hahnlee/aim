package android.os;
public class Binder implements IBinder {
    public static int uid=1000;
    public static int getCallingUid(){return uid;}
    public void attachInterface(IInterface owner,String descriptor){}
    public IInterface queryLocalInterface(String descriptor){throw new AssertionError();}
    public String getInterfaceDescriptor(){throw new AssertionError();}
    public boolean transact(int code,Parcel data,Parcel reply,int flags){throw new AssertionError();}
    protected boolean onTransact(int code,Parcel data,Parcel reply,int flags)throws RemoteException {throw new AssertionError();}
    public void dump(java.io.FileDescriptor fd,String[] args){throw new AssertionError();}
    public void shellCommand(java.io.FileDescriptor in,java.io.FileDescriptor out,java.io.FileDescriptor err,String[] args,ShellCallback callback,ResultReceiver receiver){throw new AssertionError();}
    public void linkToDeath(DeathRecipient recipient,int flags){throw new AssertionError();}
    public boolean unlinkToDeath(DeathRecipient recipient,int flags){throw new AssertionError();}
}
