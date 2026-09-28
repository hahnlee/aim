// Include order matches original hwbinder/binder_kernel.h on Darwin. The
// Linux ioctl definitions must win inside this kernel-wire translation unit.
#include <sys/ioctl.h>
#include <linux/android/binder.h>
#include <linux/sched.h>
#include <linux/sync_file.h>
#include <cstddef>
#include <cstdio>

static_assert(sizeof(binder_size_t) == 8 && sizeof(binder_uintptr_t) == 8);
static_assert(sizeof(flat_binder_object) == 24);
static_assert(sizeof(binder_fd_object) == 24);
static_assert(sizeof(binder_buffer_object) == 40);
static_assert(sizeof(binder_fd_array_object) == 32);
static_assert(sizeof(binder_transaction_data) == 64);
static_assert(offsetof(binder_transaction_data, data) == 48);
static_assert(sizeof(binder_write_read) == 48);
static_assert(BINDER_VERSION == 0xc0046209U);
static_assert(BINDER_WRITE_READ == 0xc0306201U);
static_assert(BC_TRANSACTION == 0x40406300U);
static_assert(BR_TRANSACTION == 0x80407202U);
static_assert(SCHED_NORMAL == 0 && SCHED_FIFO == 1 && SCHED_RR == 2);
static_assert(sizeof(sync_merge_data) == 48);
static_assert(sizeof(sync_fence_info) == 80);
static_assert(offsetof(sync_fence_info, timestamp_ns) == 72);
static_assert(sizeof(sync_file_info) == 56);
static_assert(offsetof(sync_file_info, sync_fence_info) == 48);
static_assert(SYNC_IOC_MERGE == 0xc0303e03U);
static_assert(SYNC_IOC_FILE_INFO == 0xc0383e04U);

int main() {
    std::puts("original ARM64 Binder UAPI structure sizes/offset and Linux ioctl values PASS");
    std::puts("original sync-file UAPI sizes/offsets and Linux ioctl values PASS");
}
