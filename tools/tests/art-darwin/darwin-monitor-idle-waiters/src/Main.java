/*
 * Threads blocked on a held monitor must park in the kernel. While the holder
 * sleeps inside the monitor, the blocked waiters' CPU time should stay near
 * zero; a spinning or polling implementation burns about one core per waiter.
 */
import java.lang.reflect.Method;
import java.util.concurrent.atomic.AtomicLong;

public class Main {
  private static final int WAITERS = 4;
  private static final long HOLD_MILLIS = 3000;
  private static final Object lock = new Object();

  public static void main(String[] args) throws Exception {
    // dalvik.system.VMDebug.threadCpuTimeNanos(): the calling thread's CPU time.
    Method threadCpu = Class.forName("dalvik.system.VMDebug").getMethod("threadCpuTimeNanos");
    AtomicLong waiterCpu = new AtomicLong();
    Thread[] waiters = new Thread[WAITERS];
    synchronized (lock) {
      for (int i = 0; i < WAITERS; i++) {
        waiters[i] = new Thread(() -> {
          synchronized (lock) {
            try {
              waiterCpu.addAndGet((Long) threadCpu.invoke(null));
            } catch (ReflectiveOperationException e) {
              throw new RuntimeException(e);
            }
          }
        });
        waiters[i].start();
      }
      Thread.sleep(HOLD_MILLIS);
    }
    for (Thread waiter : waiters) {
      waiter.join();
    }
    // Each waiter's whole lifetime is essentially the hold.
    double coresPerWaiter = waiterCpu.get() / 1e9 / (HOLD_MILLIS / 1000.0) / WAITERS;
    // A parked waiter uses a small fraction of a core; a spinning one about one.
    System.out.println(coresPerWaiter < 0.05
        ? "waiters idle"
        : "waiters busy: cores_per_waiter=" + String.format("%.4f", coresPerWaiter));
  }
}
