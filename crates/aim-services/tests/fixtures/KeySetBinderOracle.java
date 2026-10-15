public final class KeySetBinderOracle {
    private static android.os.IBinder computer(String name) { return android.os.ServiceManager.checkService(name); }
    private static android.content.pm.IPackageManager query(android.os.IBinder computer, String[] args) throws Exception {
        var data=android.os.Parcel.obtain();var reply=android.os.Parcel.obtain();
        try {
            data.writeInterfaceToken(args[1]);data.writeInt(1000);data.writeInt(android.os.Process.myPid());
            if(!computer.transact(Integer.parseInt(args[0]),data,reply,0)) throw new AssertionError("query Binder not handled");
            reply.readException();var binder=reply.readStrongBinder();
            if(binder==null || reply.dataAvail()!=0) throw new AssertionError("query Binder framing differs");
            return android.content.pm.IPackageManager.Stub.asInterface(binder);
        } finally {data.recycle();reply.recycle();}
    }
    public static void main(String[] args) {
        try {
            var first=computer("keyset_computer");var pm=query(first,args);
            var key=pm.getSigningKeySet("android");var again=pm.getSigningKeySet("android");
            if(key==null || !key.equals(again) || !key.getToken().equals(again.getToken())) throw new AssertionError("native token identity not stable");
            if(!pm.isPackageSignedByKeySet("android",key) || !pm.isPackageSignedByKeySetExactly("android",key)) throw new AssertionError("original KeySet Binder roundtrip rejected");
            if(pm.getSigningKeySet(null)!=null || pm.getKeySetByAlias("android",null)!=null || pm.isPackageSignedByKeySet("android",null)) throw new AssertionError("native nullable keysets differ");
            var foreign=new android.content.pm.KeySet(new android.os.Binder());
            if(pm.isPackageSignedByKeySet("android",foreign) || pm.isPackageSignedByKeySetExactly("android",foreign)) throw new AssertionError("foreign token accepted");
            try {pm.getKeySetByAlias("android","missing.alias");throw new AssertionError("unknown alias accepted");} catch(IllegalArgumentException expected) {}
            try {pm.getSigningKeySet("missing.package");throw new AssertionError("unknown package accepted");} catch(IllegalArgumentException expected) {}
            var replacement=query(computer("keyset_replacement"),args);var fresh=replacement.getSigningKeySet("android");
            if(fresh==null || fresh.equals(key) || replacement.isPackageSignedByKeySetExactly("android",key) || pm.isPackageSignedByKeySetExactly("android",fresh)) throw new AssertionError("foreign native owner token accepted");
            if(!replacement.isPackageSignedByKeySetExactly("android",fresh)) throw new AssertionError("replacement native token rejected");
            var data=android.os.Parcel.obtain();var reply=android.os.Parcel.obtain();
            try {data.writeInterfaceToken(args[1]);if(!first.transact(Integer.parseInt(args[2]),data,reply,0)) throw new AssertionError("close unhandled");reply.readException();} finally {data.recycle();reply.recycle();}
            try {pm.getSigningKeySet("android");throw new AssertionError("closed query accepted");} catch(IllegalStateException expected) {}
            if(!replacement.isPackageSignedByKeySetExactly("android",fresh)) throw new AssertionError("closing old lease revoked replacement");
            System.out.println("ORIGINAL_KEYSET_BINDER stable subset exact null foreign owners close");System.exit(0);
        } catch(Throwable failure) {failure.printStackTrace(System.out);System.exit(1);}
    }
}
