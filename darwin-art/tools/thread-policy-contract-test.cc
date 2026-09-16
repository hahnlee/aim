// Darwin mechanism check, independent from Android scheduling policy.
#include <mach/mach.h>
#include <mach/thread_policy.h>
#include <mach/task_policy.h>
#include <cstdio>
#include <initializer_list>
#include <cassert>

int main() {
  thread_t thread = mach_thread_self();
  for (int flavor : {THREAD_LATENCY_QOS_POLICY, THREAD_THROUGHPUT_QOS_POLICY}) {
    int value = 0;
    mach_msg_type_number_t count = 1;
    boolean_t defaults = false;
    kern_return_t get = thread_policy_get(thread, flavor, &value, &count, &defaults);
    int requested = flavor == THREAD_LATENCY_QOS_POLICY ? static_cast<int>(LATENCY_QOS_TIER_3) : static_cast<int>(THROUGHPUT_QOS_TIER_3);
    kern_return_t set = thread_policy_set(thread, flavor, &requested, 1);
    int actual = 0;
    count = 1;
    defaults = false;
    kern_return_t read = thread_policy_get(thread, flavor, &actual, &count, &defaults);
    std::printf("policy=%d get=%d original=%x set=%d read=%d actual=%x default=%d\n",
                flavor, get, value, set, read, actual, defaults);
    assert(get == KERN_SUCCESS && set == KERN_SUCCESS && read == KERN_SUCCESS);
    assert(actual == requested && defaults == false);
    assert(thread_policy_set(thread, flavor, &value, 1) == KERN_SUCCESS);
  }
  mach_port_deallocate(mach_task_self(), thread);
}
