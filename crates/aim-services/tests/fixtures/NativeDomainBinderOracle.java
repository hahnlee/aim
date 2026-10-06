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
                if (!Integer.valueOf(i == 0 ? 1 : 0).equals(states.get("h" + i + ".example"))) throw new AssertionError("native domain value differs at " + i);
            }
        } finally { data.recycle(); reply.recycle(); }
        data = android.os.Parcel.obtain(); reply = android.os.Parcel.obtain();
        try {
            data.writeInterfaceToken(args[1]); data.writeString("android"); data.writeInt(0);
            if (!service.transact(Integer.parseInt(args[2]), data, reply, 0)) throw new AssertionError("native user-state transaction unhandled");
            if (!reply.hasFileDescriptors()) throw new AssertionError("large user-state reply did not carry an FD");
            reply.readException();
            var state = reply.readTypedObject(android.content.pm.verify.domain.DomainVerificationUserState.CREATOR);
            if (reply.dataAvail() != 0 || state.getUser().getIdentifier() != 0 || !state.getPackageName().equals("android")
                || state.isLinkHandlingAllowed() != Boolean.parseBoolean(args[3])) throw new AssertionError("native user-state framing differs");
            if (state.getHostToStateMap().size() != 4000) throw new AssertionError("user-state domain count differs");
            for (int i = 0; i < 4000; i++) if (!Integer.valueOf(i == 0 ? 2 : 0).equals(state.getHostToStateMap().get("h" + i + ".example"))) throw new AssertionError("user-state value differs");
        } finally { data.recycle(); reply.recycle(); }
        for (String host : new String[] {"h0.example", "h1.example", null}) {
            data = android.os.Parcel.obtain(); reply = android.os.Parcel.obtain();
            try {
                data.writeInterfaceToken(args[1]); data.writeString(host); data.writeInt(0);
                if (!service.transact(Integer.parseInt(args[4]), data, reply, 0)) throw new AssertionError("native Owners transaction unhandled");
                if (host == null) {
                    try { reply.readException(); throw new AssertionError("null host accepted"); }
                    catch (NullPointerException expected) {}
                } else {
                    reply.readException();
                    var owners = reply.createTypedArrayList(android.content.pm.verify.domain.DomainOwner.CREATOR);
                    int count = host.equals("h0.example") ? 1 : 0;
                    if (owners == null || owners.size() != count) throw new AssertionError("native Owners count differs");
                    if (count != 0 && (!owners.get(0).getPackageName().equals("android") || owners.get(0).isOverrideable())) throw new AssertionError("native DomainOwner values differ");
                }
                if (reply.dataAvail() != 0) throw new AssertionError("native Owners framing differs");
            } finally { data.recycle(); reply.recycle(); }
        }
        for (boolean allowed : new boolean[] {false, true}) {
            data = android.os.Parcel.obtain(); reply = android.os.Parcel.obtain();
            try {
                data.writeInterfaceToken(args[1]); data.writeString("android"); data.writeBoolean(allowed); data.writeInt(0);
                if (!service.transact(Integer.parseInt(args[5]), data, reply, 0)) throw new AssertionError("native link mutation unhandled");
                reply.readException(); if (reply.dataAvail() != 0) throw new AssertionError("native link reply framing differs");
            } finally { data.recycle(); reply.recycle(); }
            data = android.os.Parcel.obtain(); reply = android.os.Parcel.obtain();
            try {
                data.writeInterfaceToken(args[1]); data.writeString("android"); data.writeInt(0);
                if (!service.transact(Integer.parseInt(args[2]), data, reply, 0)) throw new AssertionError("native post-write user state unhandled");
                reply.readException();
                var state = reply.readTypedObject(android.content.pm.verify.domain.DomainVerificationUserState.CREATOR);
                if (state.isLinkHandlingAllowed() != allowed || reply.dataAvail() != 0) throw new AssertionError("native mutation not published");
            } finally { data.recycle(); reply.recycle(); }
        }
        data = android.os.Parcel.obtain(); reply = android.os.Parcel.obtain();
        try {
            data.writeInterfaceToken(args[1]); data.writeString("android");
            data.writeStringList(java.util.Arrays.asList("h0.example", null, "h1.example", "h0.example"));
            if (!service.transact(Integer.parseInt(args[6]), data, reply, 0)) throw new AssertionError("native URI query unhandled");
            reply.readException();
            var bundle = reply.readTypedObject(android.os.Bundle.CREATOR);
            var groups = android.content.UriRelativeFilterGroup.parcelsToGroups(bundle.getParcelableArrayList("h0.example", android.content.UriRelativeFilterGroupParcel.class));
            if (bundle.keySet().size() != 1 || groups.size() != 1 || groups.get(0).getAction() != 1 || groups.get(0).getUriRelativeFilters().size() != 6 || reply.dataAvail() != 0) throw new AssertionError("native URI group framing/values differ");
            var values = new java.util.HashSet<String>();
            for (var filter : groups.get(0).getUriRelativeFilters()) values.add(filter.getUriPart() + ":" + filter.getPatternType() + ":" + filter.getFilter());
            if (!values.equals(java.util.Set.of("0:0:/path", "1:0:q=1", "2:1:fragment", "0:1:😀", "0:0:Aa", "0:0:BB"))) throw new AssertionError("native URI filter values differ");
        } finally { data.recycle(); reply.recycle(); }
        var setService = android.os.ServiceManager.checkService("query_domain_set");
        if (setService == null) throw new AssertionError("DomainSet receiver unavailable");
        for (int count : new int[] {0, 1, 4000, -1, 1}) {
            data = android.os.Parcel.obtain(); reply = android.os.Parcel.obtain();
            try {
                data.writeInterfaceToken(args[1]);
                // Large prefix forces a small inline blob on the final case.
                boolean forceBlob = count == 1 && alreadyReadInlineSet;
                data.writeString(forceBlob ? "x".repeat(20000) : "00000000-0000-0000-0000-000000000001");
                if (count == -1) {
                    data.writeInt(1); data.writeBoolean(false); data.writeInt(5);
                    for (String host : new String[] {"BB", null, "Aa", "", "BB"}) data.writeString(host);
                } else {
                    var hosts = new android.util.ArraySet<String>();
                    for (int i = 0; i < count; i++) hosts.add("h" + i + ".example");
                    data.writeTypedObject(new android.content.pm.verify.domain.DomainSet(hosts), 0);
                    if (count == 1) alreadyReadInlineSet = true;
                }
                data.writeInt(count);
                if (data.hasFileDescriptors() != (count == 4000)) throw new AssertionError("DomainSet FD form differs");
                if (!setService.transact(Integer.parseInt(args[7]), data, reply, 0)) throw new AssertionError("native DomainSet receiver unhandled");
                reply.readException();
                if (reply.readInt() != (count == -1 ? 4 : count) || reply.dataAvail() != 0) throw new AssertionError("native DomainSet values/framing differ");
            } finally { data.recycle(); reply.recycle(); }
        }
        data = android.os.Parcel.obtain(); reply = android.os.Parcel.obtain();
        String identifier;
        try {
            data.writeInterfaceToken(args[1]); data.writeString("android");
            if (!service.transact(Integer.parseInt(args[0]), data, reply, 0)) throw new AssertionError("pre-verifier Info unhandled");
            reply.readException(); identifier = reply.readTypedObject(android.content.pm.verify.domain.DomainVerificationInfo.CREATOR).getIdentifier().toString();
        } finally { data.recycle(); reply.recycle(); }
        data = android.os.Parcel.obtain(); reply = android.os.Parcel.obtain();
        try {
            var hosts = new android.util.ArraySet<String>(); for (int i = 0; i < 4000; i++) hosts.add("h" + i + ".example");
            data.writeInterfaceToken(args[1]); data.writeString(identifier);
            data.writeTypedObject(new android.content.pm.verify.domain.DomainSet(hosts), 0); data.writeInt(1);
            if (!data.hasFileDescriptors()) throw new AssertionError("verifier request did not carry FD");
            if (!service.transact(Integer.parseInt(args[7]), data, reply, 0)) throw new AssertionError("native verifier mutation unhandled");
            reply.readException(); if (reply.readInt() != 0 || reply.dataAvail() != 0) throw new AssertionError("native verifier result differs");
        } finally { data.recycle(); reply.recycle(); }
        data = android.os.Parcel.obtain(); reply = android.os.Parcel.obtain();
        try {
            data.writeInterfaceToken(args[1]); data.writeString("android");
            service.transact(Integer.parseInt(args[0]), data, reply, 0); reply.readException();
            var info = reply.readTypedObject(android.content.pm.verify.domain.DomainVerificationInfo.CREATOR);
            if (info.getHostToStateMap().size() != 4000 || reply.dataAvail() != 0) throw new AssertionError("post-verifier framing differs");
            for (int i = 0; i < 4000; i++) if (!Integer.valueOf(1).equals(info.getHostToStateMap().get("h" + i + ".example"))) throw new AssertionError("verifier state not published");
        } finally { data.recycle(); reply.recycle(); }
        System.out.println("NATIVE_DOMAIN_BINDER 4000");
    }
    private static boolean alreadyReadInlineSet;
    private NativeDomainBinderOracle() {}
}
