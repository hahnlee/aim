/** Uses the original Binder/Parcel/Info creator against the actual native DomainQueries endpoint. */
public final class NativeDomainBinderOracle {
    public static void main(String[] args) throws Exception {
        var service = android.os.ServiceManager.checkService("query_domains");
        if (service == null) throw new AssertionError("native domain endpoint unavailable");
        var data = android.os.Parcel.obtain(); var reply = android.os.Parcel.obtain();
        try {
            data.writeInterfaceToken(args[1]);
            data.writeString("android");
            if (!service.transact(Integer.parseInt(args[0]), data, reply, 0)) throw new AssertionError("native Info transaction unhandled");
            if (!reply.hasFileDescriptors()) throw new AssertionError("large native Info did not carry an FD");
            reply.readException();
            var info = reply.readTypedObject(android.content.pm.verify.domain.DomainVerificationInfo.CREATOR);
            if (!info.getPackageName().equals("android") || reply.dataAvail() != 0) throw new AssertionError("native Info framing differs");
            var states = info.getHostToStateMap();
            if (states.size() != 4000) throw new AssertionError("native domain count differs: " + states.size());
            for (int i = 0; i < 4000; i++) {
                if (!Integer.valueOf(0).equals(states.get("h" + i + ".example"))) throw new AssertionError("native domain value differs at " + i);
            }
        } finally { data.recycle(); reply.recycle(); }
        System.out.println("NATIVE_DOMAIN_BINDER 4000");
    }
    private NativeDomainBinderOracle() {}
}
