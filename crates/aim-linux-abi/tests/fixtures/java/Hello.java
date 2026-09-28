// Hello world for the ART exception on the syscall layer (tests/art.rs).
// Besides printing, it exercises what the heap reference window and the
// runtime depend on: allocation and GC, strings, a second thread, an
// exception from an implicit null check, and a hot loop for the JIT.
public class Hello {
    static int work(int n) {
        int[] a = new int[n];
        for (int i = 0; i < n; i++) {
            a[i] = i * 31 + (i >>> 3);
        }
        int s = 0;
        for (int v : a) {
            s ^= v;
        }
        return s;
    }

    public static void main(String[] args) throws Exception {
        StringBuilder sb = new StringBuilder();
        for (int i = 0; i < 1000; i++) {
            sb.append(Integer.toString(i % 10));
        }
        java.util.List<Object> keep = new java.util.ArrayList<>();
        for (int i = 0; i < 20000; i++) {
            keep.add(new byte[64]);
            if (keep.size() > 1000) {
                keep.clear();
            }
        }
        Runtime.getRuntime().gc();
        int[] result = new int[1];
        Thread t = new Thread(() -> {
            int s = 0;
            for (int i = 0; i < 2000; i++) {
                s += work(1000);
            }
            result[0] = s;
        });
        t.start();
        t.join();
        String npe;
        try {
            Object o = args.length > 100 ? new Object() : null;
            npe = o.toString();
        } catch (NullPointerException e) {
            npe = "caught";
        }
        System.out.println("hello from ART: " + sb.length() + " " + result[0] + " " + npe);
    }
}
