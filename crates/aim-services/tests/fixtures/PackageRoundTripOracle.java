import com.android.server.pm.parsing.PackageCacher;

public final class PackageRoundTripOracle {
    public static void main(String[] args) throws Exception {
        try {
            verify(args);
        } catch (Throwable failure) {
            failure.printStackTrace(System.out);
            System.exit(1);
        }
    }

    private static void verify(String[] args) throws Exception {
        var files = new java.io.File(args[0]).listFiles((dir, name) -> name.endsWith(".native"));
        if (files == null) throw new java.io.IOException("missing parcel inputs");
        java.util.Arrays.sort(files);
        for (var file : files) {
            var pkg = (com.android.internal.pm.parsing.pkg.PackageImpl)
                PackageCacher.fromCacheEntryStatic(java.nio.file.Files.readAllBytes(file.toPath()));
            if (file.getName().startsWith("scan-")) {
                verifySnapshot(file, pkg.getPackageName(), pkg.getUid());
                int uid = Integer.parseInt(file.getName().substring(5, file.getName().indexOf('.')));
                var signing = pkg.getSigningDetails();
                if (pkg.getUid() != uid || signing.getSignatureSchemeVersion() != 3
                    || signing.getSignatures().length != 1 || signing.getPublicKeys().size() != 1
                    || (uid == 10000 && signing.getPastSigningCertificates().length != 2)) {
                    throw new AssertionError("native scan UID/signing was not finalized: " + file.getName());
                }
                byte[][] certificates = null;
                int[] capabilities = null;
                try (var in = new java.io.DataInputStream(new java.io.FileInputStream(file.getPath() + ".signing"))) {
                    int count = in.readInt();
                    if (count >= 0) {
                        certificates = new byte[count][];
                        capabilities = new int[count];
                        for (int i = 0; i < count; i++) {
                            certificates[i] = in.readNBytes(in.readInt());
                            capabilities[i] = in.readInt();
                        }
                    }
                    if (in.read() != -1) throw new AssertionError("trailing signing metadata");
                }
                var restored = dev.aim.server.PackageObjects.fromCache(
                    java.nio.file.Files.readAllBytes(file.toPath()), certificates, capabilities);
                var actual = restored.getSigningDetails();
                if (uid == 10000) {
                    if (!java.util.Arrays.equals(capabilities, new int[] {21, 23})) {
                        throw new AssertionError("pinned verified GSF lineage changed: " + java.util.Arrays.toString(capabilities));
                    }
                    for (int i = 0; i < capabilities.length; i++) {
                        var historical = actual.getPastSigningCertificates()[i];
                        if (historical.getFlags() != capabilities[i]
                            || signing.getPastSigningCertificates()[i].getFlags() != 0
                            || historical == signing.getPastSigningCertificates()[i]) {
                            throw new AssertionError("lineage flags or isolation lost");
                        }
                        if (i < capabilities.length - 1) {
                            var old = new android.content.pm.SigningDetails(
                                new android.content.pm.Signature[] {historical}, 3);
                            for (int mask : new int[] {1, 2, 4, 8, 16, 32, 15, 31}) {
                                if (actual.checkCapability(old, mask)
                                        != ((capabilities[i] & mask) == mask)) {
                                    throw new AssertionError("original capability predicate differs");
                                }
                            }
                            var revoked = dev.aim.server.PackageObjects.restoreSigning(signing,
                                certificates, new int[capabilities.length]);
                            if (revoked.checkCapability(old, 1)) throw new AssertionError("revocation lost");
                        }
                    }
                    rejects(signing, certificates, null);
                    rejects(signing, certificates, new int[0]);
                    rejects(signing, null, capabilities);
                    byte[][] wrong = certificates.clone();
                    wrong[0] = new byte[] {0};
                    rejects(signing, wrong, capabilities);
                    wrong[0] = null;
                    rejects(signing, wrong, capabilities);
                    capabilities[0] = 0;
                    certificates[0][0] ^= 1;
                    actual.getSignatures()[0].setFlags(123);
                    actual.getPublicKeys().clear();
                    if (actual.getPastSigningCertificates()[0].getFlags() != 21
                        || signing.getSignatures()[0].getFlags() != 0
                        || signing.getPublicKeys().size() != 1) {
                        throw new AssertionError("mutable signing inputs leaked");
                    }
                } else if (actual.getPastSigningCertificates() != null) {
                    throw new AssertionError("absent lineage became present");
                }
            }
            if (file.getName().contains("-true.native") && (pkg.getUid() != 19001
                || !"arm64-v8a".equals(pkg.getPrimaryCpuAbi())
                || !"/data/app/fixture/lib".equals(pkg.getNativeLibraryRootDir())
                || pkg.getPageSizeAppCompatFlags() != 8)) {
                throw new AssertionError("native scan metadata was not decoded: " + file.getName());
            }
            java.nio.file.Files.write(new java.io.File(file.getPath() + ".original").toPath(),
                PackageCacher.toCacheEntryStatic(pkg));
        }
        System.out.println("PARCELS " + files.length);
    }

    private static void rejects(android.content.pm.SigningDetails signing,
            byte[][] certificates, int[] capabilities) {
        try {
            dev.aim.server.PackageObjects.restoreSigning(signing, certificates, capabilities);
        } catch (IllegalArgumentException expected) {
            return;
        }
        throw new AssertionError("inconsistent lineage accepted");
    }

    private static void verifySnapshot(java.io.File file, String name, int uid) throws Exception {
        byte[] bytes = java.nio.file.Files.readAllBytes(new java.io.File(file.getPath() + ".snapshot").toPath());
        byte[] usageBytes = java.nio.file.Files.readAllBytes(new java.io.File(file.getPath() + ".usage").toPath());
        byte[] seinfoBytes = java.nio.file.Files.readAllBytes(new java.io.File(file.getPath() + ".boot-seinfo").toPath());
        byte[] signingBytes = java.nio.file.Files.readAllBytes(new java.io.File(file.getPath() + ".saved-signing").toPath());
        var owner = new PageOwner(name, bytes, usageBytes, seinfoBytes, signingBytes);
        var stale = new PageOwner(name, bytes, usageBytes, seinfoBytes, signingBytes);
        stale.version = 2;
        try (var bad = new dev.aim.server.PackageScanLease(
                dev.aim.server.IPackageScanSnapshot.Stub.asInterface(stale))) {
            try {
                bad.getCode(name, false);
                throw new AssertionError("wrong page version accepted");
            } catch (java.io.IOException expected) {}
            try {
                bad.getUsage(name);
                throw new AssertionError("wrong usage version accepted");
            } catch (java.io.IOException expected) {}
            try {
                bad.getSigningState(name, false);
                throw new AssertionError("wrong signing version accepted");
            } catch (java.io.IOException expected) {}
            try {
                bad.getSeInfo(name);
                throw new AssertionError("wrong seInfo version accepted");
            } catch (java.io.IOException expected) {}
        }
        // Force generated Proxy/Stub parcel framing using the original Binder and Parcel.
        var endpoint = dev.aim.server.IPackageScanSnapshot.Stub.asInterface(owner);
        var lease = new dev.aim.server.PackageScanLease(endpoint);
        owner.fail = true;
        try {
            lease.getCode(name, false);
            throw new AssertionError("owner failure swallowed");
        } catch (android.os.RemoteException expected) {}
        owner.fail = false;
        owner.shortChunk = true;
        try {
            lease.getCode(name, false);
            throw new AssertionError("short chunk accepted");
        } catch (java.io.IOException expected) {}
        owner.shortChunk = false;
        var code = lease.getCode(name, false);
        int reads = owner.reads;
        if (lease.getCode(name, false) != code || owner.reads != reads
            || lease.getCode(name, true) != null || lease.getCode("missing", false) != null) {
            throw new AssertionError("capture cache or absent code differs");
        }
        var pkg = dev.aim.server.PackageObjects.fromSnapshot(code, 1, name);
        if (pkg.getUid() != uid || code.getVersion() != 1 || !code.getPackageName().equals(name)) {
            throw new AssertionError("captured package metadata differs");
        }
        if (uid == 10000 && !java.util.Arrays.equals(code.getCapabilities(), new int[] {21, 23})) {
            throw new AssertionError("transport lost lineage capabilities");
        }
        code.getCache()[0] ^= 1;
        if (code.getCertificates() != null) {
            code.getCertificates()[0][0] ^= 1;
            code.getCapabilities()[0] = 0;
        }
        var out = android.os.Parcel.obtain();
        try {
            code.writeToParcel(out, 0);
            if (!java.util.Arrays.equals(out.marshall(), bytes)) {
                throw new AssertionError("native/Java code DTO differs or getter mutation leaked");
            }
            java.nio.file.Files.write(new java.io.File(file.getPath() + ".snapshot.original").toPath(), out.marshall());
        } finally { out.recycle(); }
        try {
            dev.aim.server.PackageObjects.fromSnapshot(code, 2, name);
            throw new AssertionError("wrong capture accepted");
        } catch (IllegalArgumentException expected) {}
        try {
            dev.aim.server.PackageObjects.fromSnapshot(code, 1, "different");
            throw new AssertionError("wrong name accepted");
        } catch (IllegalArgumentException expected) {}
        owner.fail = true;
        try { lease.getUsage(name); throw new AssertionError("usage owner error swallowed"); }
        catch (android.os.RemoteException expected) {}
        owner.fail = false;
        owner.usageTail = true;
        try { lease.getUsage(name); throw new AssertionError("usage trailing bytes accepted"); }
        catch (java.io.IOException expected) {}
        owner.usageTail = false;
        try { lease.getUsage("alias"); throw new AssertionError("usage name mismatch accepted"); }
        catch (java.io.IOException expected) {}
        var usage = lease.getUsage(name);
        int usageReads = owner.usageReads;
        if (lease.getUsage(name) != usage || owner.usageReads != usageReads
                || lease.getUsage("missing") != null || usage.getVersion() != 1
                || !usage.isHistoricalAvailable()
                || !java.util.Arrays.equals(usage.getLastPackageUsageTimeInMills(), new long[]{-1,17,29,0,0,0,0,55})
                || usage.getLatestPackageUseTimeInMills() != 55
                || usage.getLatestForegroundPackageUseTimeInMills() != 29) {
            throw new AssertionError("captured usage differs");
        }
        usage.getLastPackageUsageTimeInMills()[0] = 99;
        var setting = new com.android.server.pm.PackageSetting(name, null,
                new java.io.File("/data/app/fixture"), 0, 0, new java.util.UUID(1, 1));
        setting.setAppId(uid);
        owner.fail = true;
        try { lease.getSigningState(name, false); throw new AssertionError("signing owner failure swallowed"); }
        catch (android.os.RemoteException expected) {}
        owner.fail = false;
        owner.signingTail = true;
        try { lease.getSigningState(name, false); throw new AssertionError("signing trailing bytes accepted"); }
        catch (java.io.IOException expected) {}
        owner.signingTail = false;
        try { lease.getSigningState("alias", false); throw new AssertionError("signing name mismatch accepted"); }
        catch (java.io.IOException expected) {}
        try { lease.getSigningState(name, true); throw new AssertionError("signing scope mismatch accepted"); }
        catch (java.io.IOException expected) {}
        var malformedSigning = android.os.Parcel.obtain();
        try {
            malformedSigning.writeLong(1); malformedSigning.writeString(name); malformedSigning.writeInt(uid);
            malformedSigning.writeBoolean(false); malformedSigning.writeString(null);
            malformedSigning.writeBoolean(true); malformedSigning.writeInt(3); malformedSigning.writeInt(1);
            malformedSigning.writeByteArray(new byte[]{3}); malformedSigning.writeInt(-1);
            malformedSigning.writeBoolean(false);
            owner.signingOverride = malformedSigning.marshall();
        } finally { malformedSigning.recycle(); }
        try { lease.getSigningState(name, false); throw new AssertionError("invalid signing certificate accepted"); }
        catch (IllegalArgumentException expected) {}
        owner.signingOverride = null;
        var savedSigning = lease.getSigningState(name, false);
        int signingReads = owner.signingReads;
        if (lease.getSigningState(name, false) != savedSigning || owner.signingReads != signingReads
                || lease.getSigningState("missing", false) != null || savedSigning.getAppId() != uid) {
            throw new AssertionError("saved signing lease identity");
        }
        var savedDetails = savedSigning.getPackageSigningDetails();
        var codeDetails = pkg.getSigningDetails();
        if (savedDetails.getSignatureSchemeVersion() != codeDetails.getSignatureSchemeVersion()
                || !java.util.Arrays.equals(savedDetails.getSignatures(), codeDetails.getSignatures())
                || !savedDetails.getPublicKeys().equals(codeDetails.getPublicKeys())) {
            throw new AssertionError("saved signing or derived public keys differ");
        }
        if (savedSigning.getSharedGroupName() != null) setting.setSharedUserAppId(uid);
        dev.aim.server.PackageObjects.restoreSavedSigning(setting, savedSigning, 1, false);
        var restored = setting.getSigningDetails();
        try {
            dev.aim.server.PackageObjects.restoreSavedSigning(setting, savedSigning, 2, false);
            throw new AssertionError("saved signing wrong version accepted");
        } catch (IllegalArgumentException expected) {}
        try {
            dev.aim.server.PackageObjects.restoreSavedSigning(setting, savedSigning, 1, true);
            throw new AssertionError("saved signing wrong scope accepted");
        } catch (IllegalArgumentException expected) {}
        setting.setAppId(uid + 1);
        try {
            dev.aim.server.PackageObjects.restoreSavedSigning(setting, savedSigning, 1, false);
            throw new AssertionError("saved signing wrong UID accepted");
        } catch (IllegalArgumentException expected) {}
        setting.setAppId(uid);
        if (savedSigning.getSharedGroupName() != null) {
            setting.setSharedUserAppId(uid + 1);
            try {
                dev.aim.server.PackageObjects.restoreSavedSigning(setting, savedSigning, 1, false);
                throw new AssertionError("saved signing wrong shared UID accepted");
            } catch (IllegalArgumentException expected) {}
            setting.setSharedUserAppId(uid);
        }
        if (setting.getSigningDetails() != restored) throw new AssertionError("rejected signing restore mutated setting");
        var unknownParcel = android.os.Parcel.obtain();
        try {
            unknownParcel.writeLong(1); unknownParcel.writeString(name); unknownParcel.writeInt(uid);
            unknownParcel.writeBoolean(false); unknownParcel.writeString(null);
            unknownParcel.writeBoolean(false); unknownParcel.writeBoolean(false);
            unknownParcel.setDataPosition(0);
            var unknown = dev.aim.server.PackageSigningState.CREATOR.createFromParcel(unknownParcel);
            if (unknown.getPackageSigningDetails() != android.content.pm.SigningDetails.UNKNOWN
                    || unknown.getSharedSigningDetails() != null) throw new AssertionError("unknown signing became known");
        } finally { unknownParcel.recycle(); }
        var signingOut = android.os.Parcel.obtain();
        try {
            savedSigning.writeToParcel(signingOut, 0);
            if (!java.util.Arrays.equals(signingOut.marshall(), signingBytes)) throw new AssertionError("native/Java saved signing DTO differs");
        } finally { signingOut.recycle(); }
        byte[] changedSigningBytes = java.nio.file.Files.readAllBytes(new java.io.File(file.getPath() + ".saved-signing.changed").toPath());
        var changedOwner = new PageOwner(name, bytes, usageBytes, seinfoBytes, changedSigningBytes);
        changedOwner.version = 2;
        try (var changedLease = new dev.aim.server.PackageScanLease(dev.aim.server.IPackageScanSnapshot.Stub.asInterface(changedOwner))) {
            var changed = changedLease.getSigningState(name, false);
            if (uid == 10000) {
                var original = savedSigning.getPackageSigningDetails();
                var group = changed.getSharedSigningDetails();
                var current = changed.getPackageSigningDetails();
                if (original.getPastSigningCertificates()[0].getFlags() != 21
                        || current.getPastSigningCertificates()[0].getFlags() != 20
                        || group.getPastSigningCertificates()[0].getFlags() != 17
                        || !group.getPublicKeys().equals(current.getPublicKeys())) {
                    throw new AssertionError("package/group signing owners were conflated");
                }
                var ancestor = new android.content.pm.SigningDetails(
                    new android.content.pm.Signature[]{new android.content.pm.Signature(original.getPastSigningCertificates()[0])}, 3);
                if (current.checkCapability(ancestor, 1) || !original.checkCapability(ancestor, 1)) {
                    throw new AssertionError("saved signing capabilities lost");
                }
                original.getPastSigningCertificates()[0].setFlags(0);
                if (savedSigning.getPackageSigningDetails().getPastSigningCertificates()[0].getFlags() != 21) {
                    throw new AssertionError("saved signing DTO is mutable through original getters");
                }
            }
            dev.aim.server.PackageObjects.restoreSavedSigning(setting, changed, 2, false);
        }
        owner.fail = true;
        try { lease.getSeInfo(name); throw new AssertionError("seInfo owner failure swallowed"); }
        catch (android.os.RemoteException expected) {}
        owner.fail = false;
        owner.seinfoTail = true;
        try { lease.getSeInfo(name); throw new AssertionError("seInfo trailing bytes accepted"); }
        catch (java.io.IOException expected) {}
        owner.seinfoTail = false;
        try { lease.getSeInfo("alias"); throw new AssertionError("seInfo name mismatch accepted"); }
        catch (java.io.IOException expected) {}
        var security = lease.getSeInfo(name);
        int securityReads = owner.seinfoReads;
        if (lease.getSeInfo(name) != security || owner.seinfoReads != securityReads
                || lease.getSeInfo("missing") != null) throw new AssertionError("seInfo lease identity");
        setting.getPkgState().setSeInfo("before-base");
        dev.aim.server.PackageObjects.restoreBootSeInfo(setting, security, 1);
        if (!(security.isOverride() ? security.getLabel().equals(setting.getPkgState().getOverrideSeInfo())
                    && "before-base".equals(setting.getPkgState().getSeInfo())
                : security.getLabel().equals(setting.getPkgState().getSeInfo())
                    && setting.getPkgState().getOverrideSeInfo() == null)
                || !security.getLabel().equals(((com.android.server.pm.pkg.PackageState)setting).getSeInfo())) {
            throw new AssertionError("original boot base/override differs");
        }
        setting.getPkgState().setSeInfo("stale-base");
        setting.getPkgState().setOverrideSeInfo("stale-override");
        dev.aim.server.PackageObjects.restoreSeInfo(setting, security, 1);
        if (!java.util.Objects.equals(security.getBaseLabel(), setting.getPkgState().getSeInfo())
                || !java.util.Objects.equals(security.getOverrideLabel(), setting.getPkgState().getOverrideSeInfo())
                || !security.getLabel().equals(((com.android.server.pm.pkg.PackageState)setting).getSeInfo())) {
            throw new AssertionError("original complete seInfo fields differ");
        }
        android.os.Parcel incompleteParcel = android.os.Parcel.obtain();
        try {
            incompleteParcel.writeLong(1);
            incompleteParcel.writeString(name);
            incompleteParcel.writeString(null);
            incompleteParcel.writeString("boot-only-override");
            incompleteParcel.setDataPosition(0);
            var incomplete = dev.aim.server.PackageSeInfoState.CREATOR.createFromParcel(incompleteParcel);
            try { dev.aim.server.PackageObjects.restoreSeInfo(setting, incomplete, 1);
                throw new AssertionError("missing seInfo base fabricated"); }
            catch (IllegalStateException expected) {}
            if (!java.util.Objects.equals(security.getBaseLabel(), setting.getPkgState().getSeInfo())
                    || !java.util.Objects.equals(security.getOverrideLabel(), setting.getPkgState().getOverrideSeInfo())) {
                throw new AssertionError("incomplete seInfo restoration changed an owner");
            }
        } finally { incompleteParcel.recycle(); }
        android.os.Parcel securityParcel = android.os.Parcel.obtain();
        try {
            security.writeToParcel(securityParcel, 0);
            if (!java.util.Arrays.equals(seinfoBytes, securityParcel.marshall())) {
                throw new AssertionError("seInfo native/Java bytes differ");
            }
        } finally { securityParcel.recycle(); }
        try { dev.aim.server.PackageObjects.restoreBootSeInfo(setting, security, 2);
            throw new AssertionError("wrong seInfo version restored"); }
        catch (IllegalArgumentException expected) {}
        try { dev.aim.server.PackageObjects.restoreSeInfo(setting, security, 2);
            throw new AssertionError("wrong complete seInfo version restored"); }
        catch (IllegalArgumentException expected) {}
        dev.aim.server.PackageObjects.restoreUsage(setting, usage, 1);
        if (!java.util.Arrays.equals(setting.getPkgState().getLastPackageUsageTimeInMills(),
                usage.getLastPackageUsageTimeInMills())
                || setting.getPkgState().getLatestPackageUseTimeInMills() != usage.getLatestPackageUseTimeInMills()
                || setting.getPkgState().getLatestForegroundPackageUseTimeInMills() != usage.getLatestForegroundPackageUseTimeInMills()) {
            throw new AssertionError("original usage getters differ");
        }
        var securitySetting = new com.android.server.pm.PackageSetting(name, null,
                new java.io.File("/data/app/fixture"), 1, 8, new java.util.UUID(1, 3));
        boolean readPolicy = com.android.server.pm.SELinuxMMAC.readInstallPolicy();
        String expectedRead = new String(java.nio.file.Files.readAllBytes(
                new java.io.File(file.getPath() + ".seinfo-read").toPath()), java.nio.charset.StandardCharsets.UTF_8);
        if (!Boolean.toString(readPolicy).equals(expectedRead)) {
            throw new AssertionError("original/native policy load differs: original " + readPolicy + " native " + expectedRead);
        }
        String seinfo = com.android.server.pm.SELinuxMMAC.getSeInfo(
                (com.android.server.pm.pkg.PackageState)(Object)securitySetting, pkg, true, 36);
        String nativeSeinfo = new String(java.nio.file.Files.readAllBytes(new java.io.File(file.getPath() + ".seinfo").toPath()), java.nio.charset.StandardCharsets.UTF_8);
        if (!seinfo.equals(nativeSeinfo)) throw new AssertionError("native/original seinfo differs: " + nativeSeinfo + " vs " + seinfo);
        setting.getPkgState().setLastPackageUsageTimeInMills(0, 99);
        try { dev.aim.server.PackageObjects.restoreUsage(setting, usage, 2);
            throw new AssertionError("wrong usage version restored"); }
        catch (IllegalArgumentException expected) {}
        var other = new com.android.server.pm.PackageSetting("other", null,
                new java.io.File("/data/app/other"), 0, 0, new java.util.UUID(1, 2));
        try { dev.aim.server.PackageObjects.restoreBootSeInfo(other, security, 1);
            throw new AssertionError("wrong seInfo name restored"); }
        catch (IllegalArgumentException expected) {}
        try { dev.aim.server.PackageObjects.restoreSeInfo(other, security, 1);
            throw new AssertionError("wrong complete seInfo name restored"); }
        catch (IllegalArgumentException expected) {}
        if (!security.getLabel().equals(((com.android.server.pm.pkg.PackageState)setting).getSeInfo())
                || other.getPkgState().getOverrideSeInfo() != null) {
            throw new AssertionError("rejected seInfo restoration changed an owner");
        }
        try { dev.aim.server.PackageObjects.restoreUsage(other, usage, 1);
            throw new AssertionError("wrong usage name restored"); }
        catch (IllegalArgumentException expected) {}
        if (usage.getLastPackageUsageTimeInMills()[0] != -1
                || setting.getPkgState().getLastPackageUsageTimeInMills()[0] != 99
                || other.getPkgState().getLatestPackageUseTimeInMills() != 0) {
            throw new AssertionError("usage mutation or rejected restore changed an owner");
        }
        out = android.os.Parcel.obtain();
        try {
            usage.writeToParcel(out, 0);
            if (!java.util.Arrays.equals(out.marshall(), usageBytes)) {
                throw new AssertionError("native/Java usage DTO differs");
            }
        } finally { out.recycle(); }
        lease.close();
        lease.close();
        if (owner.closes != 1) throw new AssertionError("close is not idempotent");
        try {
            lease.getCode(name, false);
            throw new AssertionError("closed lease accepted");
        } catch (IllegalStateException expected) {}
        try { lease.getUsage(name); throw new AssertionError("closed usage lease accepted"); }
        catch (IllegalStateException expected) {}
        try { lease.getSigningState(name, false); throw new AssertionError("closed signing lease accepted"); }
        catch (IllegalStateException expected) {}
        try { lease.getSeInfo(name); throw new AssertionError("closed seInfo lease accepted"); }
        catch (IllegalStateException expected) {}
    }

    private static final class PageOwner extends dev.aim.server.IPackageScanSnapshot.Stub {
        private final String name;
        private final byte[] bytes;
        private final byte[] usage;
        private final byte[] seinfo;
        private final byte[] signing;
        boolean signingTail;
        byte[] signingOverride;
        int signingReads;
        boolean seinfoTail;
        int seinfoReads;
        boolean usageTail;
        int usageReads;
        boolean fail;
        boolean shortChunk;
        int reads;
        int closes;
        long version = 1;
        PageOwner(String name, byte[] bytes, byte[] usage, byte[] seinfo, byte[] signing) { this.name = name; this.bytes = bytes; this.usage = usage; this.seinfo = seinfo; this.signing = signing; }
        @Override
        public android.os.IInterface queryLocalInterface(String descriptor) { return null; }
        @Override
        public long getVersion() { return version; }
        @Override
        public String[] getPackageNames(boolean disabled) { return disabled ? new String[0] : new String[] {name}; }
        @Override
        public int getCodeLength(String candidate, boolean disabled) { return !disabled && name.equals(candidate) ? bytes.length : -1; }
        @Override
        public byte[] getCodeChunk(String name, boolean disabled, int offset, int length) throws android.os.RemoteException {
            if (fail) throw new android.os.RemoteException();
            reads++;
            return java.util.Arrays.copyOfRange(bytes, offset, offset + length - (shortChunk ? 1 : 0));
        }
        @Override
        public void close() { closes++; }
        @Override
        public byte[] getUsage(String candidate) throws android.os.RemoteException {
            if (fail) throw new android.os.RemoteException();
            usageReads++;
            if (candidate.equals("missing")) return null;
            return usageTail ? java.util.Arrays.copyOf(usage, usage.length + 4) : usage.clone();
        }
        @Override
        public byte[] getSigningState(String candidate, boolean disabled) throws android.os.RemoteException {
            if (fail) throw new android.os.RemoteException();
            signingReads++;
            if (candidate.equals("missing")) return null;
            byte[] state = signingOverride == null ? signing : signingOverride;
            return signingTail ? java.util.Arrays.copyOf(state, state.length + 4) : state.clone();
        }
        @Override
        public byte[] getSeInfo(String candidate) throws android.os.RemoteException {
            if (fail) throw new android.os.RemoteException();
            seinfoReads++;
            if (candidate.equals("missing")) return null;
            return seinfoTail ? java.util.Arrays.copyOf(seinfo, seinfo.length + 4) : seinfo.clone();
        }
    }
}
