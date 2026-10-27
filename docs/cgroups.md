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
| `cpu.uclamp.max` <= 25% or a cpuset smaller than the online set | Background |
| `cpu.uclamp.max` <= 50% | Utility ceiling |
| `cpu.idle=1` / `cpu.shares` < 1024 | Background / utility ceiling for fair-class threads |
| `cpu.uclamp.min` > 0 or `cpu.uclamp.latency_sensitive=1` | User-initiated floor |
| `cpu.uclamp.min` >= 50% | User-interactive floor |
| `blkio.prio.class=restrict-to-be` / `idle` / `promote-to-rt` or `none-to-rt` | Utility / throttled / important disk I/O |

Ancestor uclamp bounds cap descendant requests; ceilings win over floors.
The mapping composes with each thread's nice and SCHED_BATCH/IDLE settings. When every guest thread is background,
`task_policy_set(TASK_SUPPRESSION_POLICY)` immediately constrains
the entire host task with XNU's low-priority CPU and disk policy, including
CPU-bound threads. A thread moved alone uses only thread policy: Darwin's self-only QoS, background and disk APIs
are applied when that thread next leaves a syscall, as with per-thread
nice changes. Returning to unrestricted groups clears the cgroup policy;
the thread's own Linux scheduling remains. New threads and spawned fork
children derive host policy from their inherited memberships.

The vendor init configuration sets `cpu.idle=1` for background CPU groups
and lower shares for dex2oat; the original task profiles select them.
The original's identical initial cpuset masks alone cannot distinguish
background from foreground. This is policy configuration, not a name
check in the syscall layer.

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
No FreeBSD code is copied.
