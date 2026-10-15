/** Reads native domain Parcel bytes and a native-owned ashmem FD with the original creator. */
public final class NativeAshmemOracle {
    public static void main(String[] args) throws Exception {
        try (var descriptor = android.os.ParcelFileDescriptor.fromFd(Integer.parseInt(args[0]))) {
            var parcel = android.os.Parcel.obtain();
            try {
                byte[] nativeBytes = java.nio.file.Files.readAllBytes(new java.io.File(args[1]).toPath());
                parcel.unmarshall(nativeBytes, 0, nativeBytes.length);
                // Unmarshall cannot import objects: install the passed fd at the native object's offset.
                parcel.setDataPosition(Integer.parseInt(args[2]));
                parcel.writeFileDescriptor(descriptor.getFileDescriptor());
                parcel.setDataPosition(0); parcel.readException();
                var info = parcel.readTypedObject(android.content.pm.verify.domain.DomainVerificationInfo.CREATOR);
                if (parcel.dataAvail() != 0 || !info.getPackageName().equals("fixture.large")
                    || !info.getIdentifier().toString().equals("00000000-0000-0000-0000-000000000abc"))
                    throw new AssertionError("native domain header differs");
                var states = info.getHostToStateMap();
                if (states.size() != 4000) throw new AssertionError("native domain count differs");
                for (int i = 0; i < 4000; i++) {
                    if (!Integer.valueOf(i % 5).equals(states.get("h" + i + ".example")))
                        throw new AssertionError("native domain state differs at " + i);
                }
            } finally { parcel.recycle(); }
        }
        System.out.println("NATIVE_DOMAIN_INFO 4000");
    }
    private NativeAshmemOracle() {}
}
