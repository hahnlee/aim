/** Runs the pinned libcore sort with DomainVerificationService's comparator. */
final class DomainOwnerSortOracle {
    private record Entry(int index, String name, long time) {}
    static void write(java.io.File directory) throws Exception {
        var out = android.os.Parcel.obtain();
        try {
            int[] sizes = {0, 1, 2, 3, 15, 31, 32, 33, 64, 128};
            long[] edge = {0, 1, -1, Integer.MAX_VALUE, Integer.MIN_VALUE,
                1L << 32, -(1L << 32), Long.MIN_VALUE, Long.MAX_VALUE};
            out.writeInt(sizes.length * 6 * 16);
            for (int size : sizes) for (int mode = 0; mode < 6; mode++) for (int seed = 0; seed < 16; seed++) {
                var entries = new java.util.ArrayList<Entry>();
                var random = new java.util.Random(seed);
                for (int i = 0; i < size; i++) {
                    long time = switch (mode) {
                        case 0 -> i;
                        case 1 -> 0;
                        case 2 -> edge[random.nextInt(edge.length)];
                        case 3 -> random.nextLong();
                        case 4 -> (long) i << 32;
                        default -> (long) (i % 3) * 0x60000000L;
                    };
                    String name = (i % 2 == 0 ? "fixture.A" : "fixture.a") + (i / 2);
                    entries.add(new Entry(i, name, time));
                }
                java.util.Collections.shuffle(entries, random);
                out.writeInt(mode); out.writeInt(size);
                for (var entry : entries) {
                    out.writeInt(entry.index()); out.writeString(entry.name()); out.writeLong(entry.time());
                }
                try {
                    entries.sort((first, second) -> first.time() != second.time()
                        ? (int) (first.time() - second.time())
                        : first.name().compareToIgnoreCase(second.name()));
                    out.writeInt(0);
                } catch (IllegalArgumentException error) {
                    out.writeInt(1);
                }
                for (var entry : entries) out.writeInt(entry.index());
            }
            java.nio.file.Files.write(new java.io.File(directory, "domain-owner-sort.original").toPath(), out.marshall());
        } finally { out.recycle(); }
    }
    private DomainOwnerSortOracle() {}
}
