# Cgroups and Darwin scheduling

The pinned image uses **v1** cpu, cpuset and blkio controllers and the
**v2** unified process hierarchy (`system/etc/cgroups.json` and
`task_profiles.json`). A controller cannot be bound to both versions.
The layer owns shared membership, attachment credentials, control-file
validation and inheritance; the target process's existing agent applies
host policy. No runtime decision depends on a group or application name.

The vendor mapping is deliberately coarse: Darwin offers scheduling
classes, not Linux CFS shares, utilization percentages or arm64 CPU binding.

| Effective controller setting (including ancestors) | Darwin policy |
| --- | --- |
| `cpu.uclamp.max` <= 50% | Remove the cgroup boost, preserving Linux scheduling priority |
| cpuset masks, including a subset of the online set | Preserve scheduling and disk policy; Darwin has no arm64 CPU binding API |
| `cpu.idle=1` / `cpu.shares` < 1024 | Background / utility ceiling for fair-class threads |
| `cpu.uclamp.min` > 0 or `cpu.uclamp.latency_sensitive=1` | User-initiated floor |
| `cpu.uclamp.min` >= 50% | User-interactive floor |
| `blkio.prio.class=restrict-to-be` / `idle` / `promote-to-rt` or `none-to-rt` | Utility / throttled / important disk I/O |

Ancestor uclamp bounds cap descendant requests; ceilings win over floors.
Uclamp and cpusets guide Linux frequency/placement, not scheduling class;
neither can select Darwin Background or Utility or throttle disk I/O.
The mapping composes with each thread's nice and SCHED_BATCH/IDLE settings. When every guest thread is background,
`task_policy_set(TASK_SUPPRESSION_POLICY)` immediately constrains
the entire host task with XNU's low-priority CPU and disk policy, including
CPU-bound threads. A thread moved alone uses only thread policy: Darwin's self-only QoS, background and disk APIs
are applied when that thread next leaves a syscall, as with per-thread
nice changes. The priority agent (opcode 22) retains the target's
owner/RLIMIT_NICE checks and changes its nice value independently of these
bounds; cgroup policy uses the separate opcode 23. Task suppression changes
mark every guest thread for reapplication against the task's new base priority.
Returning to unrestricted groups clears the cgroup policy;
the thread's own Linux scheduling remains. New threads and spawned fork
children derive host policy from their inherited memberships.

Non-default disk policies require explicit Darwin thread scheduling: XNU
removes QoS when they are set, and a subsequent QoS request fails with
EPERM. The layer leaves QoS before setting disk policy and keeps using
explicit scheduling when the disk policy returns to its default. CPU
priority still follows nice and the CPU controller bounds independently.
This matters to logd's startup `SP_BACKGROUND` profile, which joins both
cpu/background and blkio/background; a failed profile aborts logd before
audio services can publish their logs.

The vendor init configuration sets `cpu.idle=1` for background CPU groups
and lower shares for dex2oat; the original task profiles select them.
The original's identical initial cpuset masks alone cannot distinguish
background from foreground. This is policy configuration, not a name
check in the syscall layer.

The pinned audioserver and the vendor audio HAL request
`ProcessCapacityHigh HighPerformance`: `task_profiles.json` selects
cpuset/foreground and cpu/foreground. Init copies the root CPU mask and
leaves CPU shares at 1024, idle at 0 and uclamp at 0/max; neither profile
changes blkio's neutral root policy. These values preserve normal, negative
nice and real-time thread scheduling. Smaller CPU masks or lower utilization
maxima likewise cannot demote those scheduling classes. The CoreAudio helper
enters `--audio-io` before guest/cgroup initialization.

The pinned `SCHED_SP_RT_APP` profile joins cpu/rt and the neutral blkio
root. The group name grants no special priority: FIFO/RR retain their
scheduling class, independent of CFS shares/idle and uclamp performance
hints. AIM does not advertise CONFIG_RT_GROUP_SCHED or expose
`cpu.rt_runtime_us`/`cpu.rt_period_us`, so it imposes no per-group RT
bandwidth admission check. On Linux with RT group scheduling enabled,
non-root groups instead need an allocated runtime before accepting RT
tasks; a zero runtime is not a CFS priority ceiling.

The unified hierarchy exposes process membership, population and empty
controller/subtree-control lists; migrations check the common ancestor's
write permission using the control file opener's saved credentials. CPU/cpuset/I/O stay bound to
v1, and no memory controller is advertised. Freezing is unavailable:
`task_suspend` would also suspend the in-process agent needed to unfreeze;
selective `thread_suspend` cannot preserve Linux freezer wakeup, fork and
exit semantics. `cgroup.freeze` is absent and opening it returns ENOENT,
as on Linux built without CONFIG_CGROUP_FREEZER. Kernfs rejects attempts
to create it with EACCES. No SIGSTOP is sent and no freeze reports success.
CFS bandwidth files likewise remain absent.

Semantics follow [cgroups(7)](https://man7.org/linux/man-pages/man7/cgroups.7.html),
the [Linux cgroup v2 ABI](https://docs.kernel.org/admin-guide/cgroup-v2.html)
and the android16-6.12 controller implementations. Host operations follow
[XNU's resource policies](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/kern_resource.c).
RT behavior follows [Linux RT group scheduling](https://docs.kernel.org/scheduler/sched-rt-group.html)
and [utilization clamping](https://docs.kernel.org/scheduler/sched-util-clamp.html).
No FreeBSD code is copied.
