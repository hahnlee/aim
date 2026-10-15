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
        for (boolean enabled : new boolean[] {true, false}) {
            data = android.os.Parcel.obtain(); reply = android.os.Parcel.obtain();
            try {
                var hosts = new android.util.ArraySet<String>();
                if (enabled) { hosts.add("h1.example"); hosts.add("h2.example"); }
                else for (int i = 0; i < 4000; i++) hosts.add("h" + i + ".example");
                data.writeInterfaceToken(args[1]); data.writeString(identifier); data.writeTypedObject(new android.content.pm.verify.domain.DomainSet(hosts), 0);
                data.writeBoolean(enabled); data.writeInt(0);
                if (data.hasFileDescriptors() != !enabled) throw new AssertionError("selection FD form differs");
                if (!service.transact(Integer.parseInt(args[8]), data, reply, 0)) throw new AssertionError("native selection unhandled");
                reply.readException(); if (reply.readInt() != 0 || reply.dataAvail() != 0) throw new AssertionError("selection status differs");
            } finally { data.recycle(); reply.recycle(); }
            data = android.os.Parcel.obtain(); reply = android.os.Parcel.obtain();
            try {
                data.writeInterfaceToken(args[1]); data.writeString("android"); data.writeInt(0);
                service.transact(Integer.parseInt(args[2]), data, reply, 0); reply.readException();
                var selection = reply.readTypedObject(android.content.pm.verify.domain.DomainVerificationUserState.CREATOR);
                if (!Integer.valueOf(enabled ? 1 : 0).equals(selection.getHostToStateMap().get("h1.example")) || !Integer.valueOf(enabled ? 1 : 0).equals(selection.getHostToStateMap().get("h2.example")) || reply.dataAvail() != 0) throw new AssertionError("selection not published");
            } finally { data.recycle(); reply.recycle(); }
        }
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
        data = android.os.Parcel.obtain(); reply = android.os.Parcel.obtain();
        try {
            var bundle = new android.os.Bundle(); var groups = new java.util.ArrayList<android.content.UriRelativeFilterGroupParcel>();
            var group = new android.content.UriRelativeFilterGroupParcel(); group.action = 99; group.filters = new java.util.ArrayList<>();
            var filter = new android.content.UriRelativeFilterParcel(); filter.uriPart = 2; filter.patternType = 1; filter.filter = null; group.filters.add(filter); groups.add(group);
            bundle.putParcelableArrayList("runtime.example", groups);
            // Non-URI Binder value must be a typed-getter mismatch, not a bad Parcel.
            bundle.putBinder("h0.example", service);
            data.writeInterfaceToken(args[1]); data.writeString("android"); data.writeTypedObject(bundle, 0);
            if (!service.transact(Integer.parseInt(args[9]), data, reply, 0)) throw new AssertionError("URI update unhandled");
            reply.readException(); if (reply.dataAvail() != 0) throw new AssertionError("URI update framing differs");
        } finally { data.recycle(); reply.recycle(); }
        data = android.os.Parcel.obtain(); reply = android.os.Parcel.obtain();
        var styled = android.os.Parcel.obtain(); var styledWire = android.os.Parcel.obtain();
        try {
            styled.writeInt(1); styled.writeString("h0.example"); styled.writeInt(10);
            styled.writeInt(0); styled.writeString8("styled");
            styled.writeInt(2); styled.writeInt(0xff123456); // ForegroundColorSpan
            styled.writeInt(0); styled.writeInt(6); styled.writeInt(33); styled.writeInt(0);
            int length = styled.marshall().length;
            styledWire.writeInt(length); styledWire.writeInt(0x4c444e42);
            styledWire.appendFrom(styled, 0, length); styledWire.writeBoolean(false); styledWire.setDataPosition(0);
            var bundle = android.os.Bundle.CREATOR.createFromParcel(styledWire);
            data.writeInterfaceToken(args[1]); data.writeString("android"); data.writeTypedObject(bundle, 0);
            if (!service.transact(Integer.parseInt(args[9]), data, reply, 0)) throw new AssertionError("styled URI update unhandled");
            reply.readException(); if (reply.dataAvail() != 0) throw new AssertionError("styled URI update framing differs");
        } finally {styled.recycle(); styledWire.recycle(); data.recycle(); reply.recycle();}
        String[] conversionErrors = {
            "Attempt to read from field 'int android.content.UriRelativeFilterGroupParcel.action' on a null object reference in method 'void android.content.UriRelativeFilterGroup.<init>(android.content.UriRelativeFilterGroupParcel)'",
            "Attempt to invoke interface method 'int java.util.List.size()' on a null object reference",
            "Attempt to read from field 'int android.content.UriRelativeFilterParcel.uriPart' on a null object reference in method 'void android.content.UriRelativeFilter.<init>(android.content.UriRelativeFilterParcel)'"
        };
        for (int mode = 0; mode < 4; mode++) {
            data = android.os.Parcel.obtain(); reply = android.os.Parcel.obtain();
            try {
                var bundle = new android.os.Bundle(); var groups = new java.util.ArrayList<android.content.UriRelativeFilterGroupParcel>();
                var group = new android.content.UriRelativeFilterGroupParcel();
                if (mode == 0) groups.add(null);
                else {if (mode == 2) {group.filters = new java.util.ArrayList<>(); group.filters.add(null);} groups.add(group);}
                bundle.putParcelableArrayList("runtime.example", groups);
                if (mode == 3) bundle = nullNamedBundle("runtime.example");
                data.writeInterfaceToken(args[1]); data.writeString("android"); data.writeTypedObject(bundle, 0);
                if (!service.transact(Integer.parseInt(args[9]), data, reply, 0)) throw new AssertionError("URI conversion error unhandled");
                try {reply.readException(); throw new AssertionError("null URI conversion accepted");}
                catch (NullPointerException error) {if (!conversionErrors[mode == 3 ? 0 : mode].equals(error.getMessage())) throw new AssertionError("URI conversion message differs: " + error.getMessage());}
                if (reply.dataAvail() != 0) throw new AssertionError("URI conversion error framing differs");
            } finally {data.recycle(); reply.recycle();}
        }
        data = android.os.Parcel.obtain(); reply = android.os.Parcel.obtain();
        try {
            var bundle = new android.os.Bundle();
            var group = new android.content.UriRelativeFilterGroupParcel(); group.action = 1; group.filters = new java.util.ArrayList<>();
            var groups = new java.util.ArrayList<android.content.UriRelativeFilterGroupParcel>(); groups.add(group);
            var bad = new java.util.ArrayList<android.content.UriRelativeFilterGroupParcel>(); bad.add(null);
            bundle.putParcelableArrayList("late.example", groups); bundle.putParcelableArrayList("runtime.example", bad); bundle.putParcelableArrayList("partial.example", groups);
            data.writeInterfaceToken(args[1]); data.writeString("android"); data.writeTypedObject(bundle, 0);
            service.transact(Integer.parseInt(args[9]), data, reply, 0);
            try {reply.readException(); throw new AssertionError("partial URI conversion accepted");}
            catch (NullPointerException error) {if (!conversionErrors[0].equals(error.getMessage())) throw new AssertionError("partial URI failure message differs");}
            if (reply.dataAvail() != 0) throw new AssertionError("partial URI error framing differs");
        } finally {data.recycle(); reply.recycle();}
        data = android.os.Parcel.obtain(); reply = android.os.Parcel.obtain();
        try {
            data.writeInterfaceToken(args[1]); data.writeString("android"); data.writeStringList(java.util.List.of("partial.example", "runtime.example", "late.example"));
            service.transact(Integer.parseInt(args[6]), data, reply, 0); reply.readException();
            var bundle = reply.readTypedObject(android.os.Bundle.CREATOR);
            var groups = bundle.getParcelableArrayList("partial.example", android.content.UriRelativeFilterGroupParcel.class);
            var existing = bundle.getParcelableArrayList("runtime.example", android.content.UriRelativeFilterGroupParcel.class);
            if (groups == null || groups.size() != 1 || groups.get(0).action != 1 || !groups.get(0).filters.isEmpty() || existing == null || existing.get(0).action != 99 || bundle.keySet().contains("late.example") || reply.dataAvail() != 0) throw new AssertionError("partial URI mutation order differs");
        } finally {data.recycle(); reply.recycle();}
        data = android.os.Parcel.obtain(); reply = android.os.Parcel.obtain();
        try {
            data.writeInterfaceToken(args[1]); data.writeString("android"); data.writeStringList(java.util.List.of("runtime.example"));
            service.transact(Integer.parseInt(args[6]), data, reply, 0); reply.readException();
            var bundle = reply.readTypedObject(android.os.Bundle.CREATOR);
            var groups = bundle.getParcelableArrayList("runtime.example", android.content.UriRelativeFilterGroupParcel.class);
            if (groups.size() != 1 || groups.get(0).action != 99 || groups.get(0).filters.size() != 1 || groups.get(0).filters.get(0).filter != null || reply.dataAvail() != 0) throw new AssertionError("nullable URI update not preserved");
        } finally { data.recycle(); reply.recycle(); }
        data = android.os.Parcel.obtain(); reply = android.os.Parcel.obtain();
        try {
            data.writeInterfaceToken(args[1]); data.writeString("android"); data.writeStringList(java.util.List.of("h0.example", "runtime.example"));
            service.transact(Integer.parseInt(args[6]), data, reply, 0); reply.readException();
            var bundle = reply.readTypedObject(android.os.Bundle.CREATOR);
            if (bundle.keySet().contains("h0.example") || !bundle.keySet().contains("runtime.example") || reply.dataAvail() != 0) throw new AssertionError("Binder mismatch/removal or later URI key differs");
        } finally {data.recycle(); reply.recycle();}
        data = android.os.Parcel.obtain(); reply = android.os.Parcel.obtain();
        try {
            data.writeStrongBinder(new android.os.Binder() {
                @Override protected boolean onTransact(int code, android.os.Parcel data, android.os.Parcel reply, int flags) {
                    if (code == 1) throw new StringIndexOutOfBoundsException("length=1; index=1");
                    if (code == 2) throw new IllegalArgumentException("invalid probe pattern");
                    return false;
                }
            });
            android.os.ServiceManager.checkService("query_domain_set").transact(1717, data, reply, 0);
            reply.readException();
            if (reply.readInt() != -74 || reply.dataAvail() != 0) throw new AssertionError("original non-serializable Binder status differs");
        } finally {data.recycle(); reply.recycle();}
        for (String key : new String[] {null, ""}) {
            data = android.os.Parcel.obtain(); reply = android.os.Parcel.obtain();
            try {
                var bundle = new android.os.Bundle();
                var group = new android.content.UriRelativeFilterGroupParcel(); group.action = 1; group.filters = new java.util.ArrayList<>();
                var groups = new java.util.ArrayList<android.content.UriRelativeFilterGroupParcel>(); groups.add(group);
                bundle.putParcelableArrayList("runtime.example", groups); bundle.putString(key, "wrong type"); bundle.putParcelableArrayList("a.example", groups);
                data.writeInterfaceToken(args[1]); data.writeString("android"); data.writeTypedObject(bundle, 0);
                boolean handled = service.transact(Integer.parseInt(args[9]), data, reply, 0);
                if (key == null) {
                    if (!handled) throw new AssertionError("null URI key unhandled");
                    try {reply.readException(); throw new AssertionError("null URI key accepted");}
                    catch (NullPointerException error) {if (!"Attempt to invoke virtual method 'int java.lang.String.length()' on a null object reference".equals(error.getMessage())) throw new AssertionError("null key message differs");}
                } else if (handled) throw new AssertionError("empty URI key did not return UNKNOWN_TRANSACTION");
                if (reply.dataAvail() != 0) throw new AssertionError("key error framing differs");
            } finally {data.recycle(); reply.recycle();}
            data = android.os.Parcel.obtain(); reply = android.os.Parcel.obtain();
            try {
                data.writeInterfaceToken(args[1]); data.writeString("android"); data.writeStringList(java.util.List.of("a.example", "runtime.example"));
                service.transact(Integer.parseInt(args[6]), data, reply, 0); reply.readException();
                var bundle = reply.readTypedObject(android.os.Bundle.CREATOR);
                if (bundle.getParcelableArrayList("a.example", android.content.UriRelativeFilterGroupParcel.class).get(0).action != 1 || bundle.getParcelableArrayList("runtime.example", android.content.UriRelativeFilterGroupParcel.class).get(0).action != 99 || reply.dataAvail() != 0) throw new AssertionError("key error partial mutation differs");
            } finally {data.recycle(); reply.recycle();}
        }
        for (String name : new String[] {"dev.aim.fixture.NoSuch", "java.lang.String", "android.content.UriRelativeFilterParcel", "android.os.Bundle", "[I"}) for (boolean root : new boolean[] {false, true}) {
            data = android.os.Parcel.obtain(); reply = android.os.Parcel.obtain();
            try {
                var seed = new android.os.Bundle(); var group = new android.content.UriRelativeFilterGroupParcel(); group.action = 1; group.filters = new java.util.ArrayList<>();
                var groups = new java.util.ArrayList<android.content.UriRelativeFilterGroupParcel>(); groups.add(group); seed.putParcelableArrayList("class.example", groups);
                data.writeInterfaceToken(args[1]); data.writeString("android"); data.writeTypedObject(seed, 0);
                service.transact(Integer.parseInt(args[9]), data, reply, 0); reply.readException();
            } finally {data.recycle(); reply.recycle();}
            boolean mismatch = name.equals("android.content.UriRelativeFilterParcel") || name.equals("android.os.Bundle");
            data = android.os.Parcel.obtain(); reply = android.os.Parcel.obtain();
            try {
                data.writeInterfaceToken(args[1]); data.writeString("android"); data.writeTypedObject(namedBundle("class.example", name, root), 0);
                service.transact(Integer.parseInt(args[9]), data, reply, 0);
                try {reply.readException(); if (!mismatch) throw new AssertionError("bad Parcelable class accepted");}
                catch (RuntimeException error) {
                    String expected = name.equals("dev.aim.fixture.NoSuch") ? "ClassNotFoundException when unmarshalling: " + name : "Parcelable protocol requires subclassing from Parcelable on class " + name;
                    if (mismatch || !"android.os.BadParcelableException".equals(error.getClass().getName()) || !expected.equals(error.getMessage())) throw error;
                }
                if (reply.dataAvail() != 0) throw new AssertionError("Parcelable error framing differs");
            } finally {data.recycle(); reply.recycle();}
            data = android.os.Parcel.obtain(); reply = android.os.Parcel.obtain();
            try {
                data.writeInterfaceToken(args[1]); data.writeString("android"); data.writeStringList(java.util.List.of("class.example"));
                service.transact(Integer.parseInt(args[6]), data, reply, 0); reply.readException();
                if (reply.readTypedObject(android.os.Bundle.CREATOR).keySet().contains("class.example") == mismatch || reply.dataAvail() != 0) throw new AssertionError("Parcelable mismatch/removal/error state differs");
            } finally {data.recycle(); reply.recycle();}
        }
        for (String name : new String[] {null, "dev.aim.fixture.NoSuch", "java.lang.String"}) for (boolean root : new boolean[] {false, true}) {
            data = android.os.Parcel.obtain(); reply = android.os.Parcel.obtain();
            try {
                var seed = new android.os.Bundle(); var group = new android.content.UriRelativeFilterGroupParcel(); group.action = 1; group.filters = new java.util.ArrayList<>();
                var groups = new java.util.ArrayList<android.content.UriRelativeFilterGroupParcel>(); groups.add(group); seed.putParcelableArrayList("serial.example", groups);
                data.writeInterfaceToken(args[1]); data.writeString("android"); data.writeTypedObject(seed, 0); service.transact(Integer.parseInt(args[9]), data, reply, 0); reply.readException();
            } finally {data.recycle(); reply.recycle();}
            boolean errorExpected = name != null || !root;
            data = android.os.Parcel.obtain(); reply = android.os.Parcel.obtain();
            try {
                data.writeInterfaceToken(args[1]); data.writeString("android"); data.writeTypedObject(serialBundle("serial.example", name, root), 0); service.transact(Integer.parseInt(args[9]), data, reply, 0);
                try {reply.readException(); if (errorExpected) throw new AssertionError("malformed Serializable accepted");}
                catch (RuntimeException error) {
                    String expectedClass = name == null ? "java.lang.NullPointerException" : "android.os.BadParcelableException";
                    String expectedMessage = name == null ? conversionErrors[0] : "Parcelable encountered IOException reading a Serializable object (name = " + name + ")";
                    if (!errorExpected || !expectedClass.equals(error.getClass().getName()) || !expectedMessage.equals(error.getMessage())) throw error;
                }
                if (reply.dataAvail() != 0) throw new AssertionError("Serializable error framing differs");
            } finally {data.recycle(); reply.recycle();}
            data = android.os.Parcel.obtain(); reply = android.os.Parcel.obtain();
            try {
                data.writeInterfaceToken(args[1]); data.writeString("android"); data.writeStringList(java.util.List.of("serial.example")); service.transact(Integer.parseInt(args[6]), data, reply, 0); reply.readException();
                if (reply.readTypedObject(android.os.Bundle.CREATOR).keySet().contains("serial.example") != errorExpected || reply.dataAvail() != 0) throw new AssertionError("Serializable error/null state differs");
            } finally {data.recycle(); reply.recycle();}
        }
        Object[] validValues = {"wrong", new java.util.ArrayList<>(), new java.util.ArrayList<>(java.util.Arrays.asList((Object) null)), new java.util.ArrayList<>(java.util.List.of("wrong"))};
        for (int mode = 0; mode < validValues.length; mode++) for (boolean root : new boolean[] {false, true}) {
            data = android.os.Parcel.obtain(); reply = android.os.Parcel.obtain();
            try {
                var seed = new android.os.Bundle(); var group = new android.content.UriRelativeFilterGroupParcel(); group.action = 1; group.filters = new java.util.ArrayList<>(); var groups = new java.util.ArrayList<android.content.UriRelativeFilterGroupParcel>(); groups.add(group); seed.putParcelableArrayList("validserial.example", groups);
                data.writeInterfaceToken(args[1]); data.writeString("android"); data.writeTypedObject(seed, 0); service.transact(Integer.parseInt(args[9]), data, reply, 0); reply.readException();
            } finally {data.recycle(); reply.recycle();}
            boolean errorExpected = root && mode >= 2;
            var bytes = new java.io.ByteArrayOutputStream(); try (var stream = new java.io.ObjectOutputStream(bytes)) {stream.writeObject(validValues[mode]);}
            data = android.os.Parcel.obtain(); reply = android.os.Parcel.obtain();
            try {
                data.writeInterfaceToken(args[1]); data.writeString("android"); data.writeTypedObject(serialPayloadBundle("validserial.example", validValues[mode].getClass().getName(), bytes.toByteArray(), root), 0);
                boolean handled = service.transact(Integer.parseInt(args[9]), data, reply, 0);
                if (root && mode == 3) {if (handled) throw new AssertionError("serialized ClassCast did not return UNKNOWN_TRANSACTION");}
                else {if (!handled) throw new AssertionError("valid serialized value unhandled"); try {reply.readException(); if (errorExpected) throw new AssertionError("serialized null element accepted");} catch (NullPointerException error) {if (!errorExpected || !conversionErrors[0].equals(error.getMessage())) throw error;}}
                if (reply.dataAvail() != 0) throw new AssertionError("valid Serializable framing differs");
            } finally {data.recycle(); reply.recycle();}
            data = android.os.Parcel.obtain(); reply = android.os.Parcel.obtain();
            try {
                data.writeInterfaceToken(args[1]); data.writeString("android"); data.writeStringList(java.util.List.of("validserial.example")); service.transact(Integer.parseInt(args[6]), data, reply, 0); reply.readException();
                if (reply.readTypedObject(android.os.Bundle.CREATOR).keySet().contains("validserial.example") != errorExpected || reply.dataAvail() != 0) throw new AssertionError("valid Serializable state differs");
            } finally {data.recycle(); reply.recycle();}
        }
        for (String alias : new String[] {"query_uri_bounds", "query_uri_invalid"}) {
            data = android.os.Parcel.obtain(); reply = android.os.Parcel.obtain();
            try {
                data.writeInterfaceToken("android.content.pm.IPackageManager"); data.writeInt(1);
                data.writeString8("android.intent.action.VIEW"); data.writeInt(1); data.writeString8("https://x/path");
                data.writeString8(null); data.writeString8(null); data.writeInt(0); data.writeInt(0);
                data.writeString8(null); data.writeString(null);
                for (int value : new int[] {0, 0, 0, 0, -2, -1, 0, 0}) data.writeInt(value);
                data.writeString(null); data.writeLong(0xc0000L); data.writeInt(0);
                boolean handled = android.os.ServiceManager.checkService(alias).transact(Integer.parseInt(args[10]), data, reply, 0);
                if (alias.equals("query_uri_bounds")) {
                    if (handled || reply.dataAvail() != 0) throw new AssertionError("native bounds did not return original UNKNOWN_TRANSACTION");
                } else {
                    if (!handled) throw new AssertionError("native invalid-pattern query unhandled");
                    try {reply.readException(); throw new AssertionError("invalid pattern accepted");}
                    catch (IllegalArgumentException error) {if (!"Modifier must follow a token.".equals(error.getMessage())) throw new AssertionError("native invalid pattern message differs");}
                    if (reply.dataAvail() != 0) throw new AssertionError("native invalid-pattern error framing differs");
                }
            } finally {data.recycle(); reply.recycle();}
        }
        System.out.println("NATIVE_DOMAIN_BINDER 4000");
    }
    private static boolean alreadyReadInlineSet;
    private static android.os.Bundle nullNamedBundle(String key) {return namedBundle(key, null);}
    private static android.os.Bundle namedBundle(String key, String name) {return namedBundle(key, name, false);}
    private static android.os.Bundle namedBundle(String key, String name, boolean root) {
        var value = android.os.Parcel.obtain(); var list = android.os.Parcel.obtain(); var body = android.os.Parcel.obtain(); var wire = android.os.Parcel.obtain();
        try {
            value.writeString(name); int length = value.marshall().length;
            body.writeInt(1); body.writeString(key); body.writeInt(root ? 4 : 11);
            if (root) {body.writeInt(length); body.appendFrom(value, 0, length);}
            else {list.writeInt(1); list.writeInt(4); list.writeInt(length); list.appendFrom(value, 0, length); length = list.marshall().length; body.writeInt(length); body.appendFrom(list, 0, length);}
            length = body.marshall().length; wire.writeInt(length); wire.writeInt(0x4c444e42); wire.appendFrom(body, 0, length); wire.writeBoolean(false); wire.setDataPosition(0);
            return android.os.Bundle.CREATOR.createFromParcel(wire);
        } finally {value.recycle(); list.recycle(); body.recycle(); wire.recycle();}
    }
    private static android.os.Bundle serialBundle(String key, String name, boolean root) {return serialPayloadBundle(key, name, new byte[] {0}, root);}
    private static android.os.Bundle serialPayloadBundle(String key, String name, byte[] payload, boolean root) {
        var value = android.os.Parcel.obtain(); var list = android.os.Parcel.obtain(); var body = android.os.Parcel.obtain(); var wire = android.os.Parcel.obtain();
        try {
            value.writeString(name); if (name != null) value.writeByteArray(payload);
            int length = value.marshall().length; body.writeInt(1); body.writeString(key); body.writeInt(root ? 21 : 11);
            if (root) {body.writeInt(length); body.appendFrom(value, 0, length);}
            else {list.writeInt(1); list.writeInt(21); list.writeInt(length); list.appendFrom(value, 0, length); length = list.marshall().length; body.writeInt(length); body.appendFrom(list, 0, length);}
            length = body.marshall().length; wire.writeInt(length); wire.writeInt(0x4c444e42); wire.appendFrom(body, 0, length); wire.writeBoolean(false); wire.setDataPosition(0);
            return android.os.Bundle.CREATOR.createFromParcel(wire);
        } finally {value.recycle(); list.recycle(); body.recycle(); wire.recycle();}
    }
    private NativeDomainBinderOracle() {}
}
