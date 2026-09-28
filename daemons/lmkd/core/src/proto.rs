// SPDX-License-Identifier: Apache-2.0
// Ported from AOSP lmkd (platform/system/memory/lmkd at android-16.0.0_r1:
// include/lmkd.h, statslog.h, statslog.cpp), Copyright 2018 Google, Inc,
// Apache-2.0.

//! The lmkd control protocol (`lmkd.h`): packets of big-endian 32-bit
//! words, the command first, over a `SOCK_SEQPACKET` socket.

pub const LMK_TARGET: i32 = 0;
pub const LMK_PROCPRIO: i32 = 1;
pub const LMK_PROCREMOVE: i32 = 2;
pub const LMK_PROCPURGE: i32 = 3;
pub const LMK_GETKILLCNT: i32 = 4;
pub const LMK_SUBSCRIBE: i32 = 5;
/// Unsolicited, to clients subscribed to [`LMK_ASYNC_EVENT_KILL`].
pub const LMK_PROCKILL: i32 = 6;
pub const LMK_UPDATE_PROPS: i32 = 7;
/// Unsolicited, to clients subscribed to [`LMK_ASYNC_EVENT_STAT`].
pub const LMK_STAT_KILL_OCCURRED: i32 = 8;
pub const LMK_START_MONITORING: i32 = 9;
pub const LMK_BOOT_COMPLETED: i32 = 10;
pub const LMK_PROCS_PRIO: i32 = 11;

/// `MAX_TARGETS`: minfree/oom_score_adj pairs in an [`LMK_TARGET`].
pub const MAX_TARGETS: usize = 6;
/// `CTRL_PACKET_MAX_SIZE`: the largest control packet, in bytes.
pub const CTRL_PACKET_MAX_SIZE: usize = 4 * (MAX_TARGETS * 2 + 1);
/// `LMK_PROCPRIO_FIELD_COUNT`: pid, uid, oom_score_adj, process type.
pub const PROCPRIO_FIELDS: usize = 4;

/// `async_event_type`.
pub const LMK_ASYNC_EVENT_KILL: i32 = 0;
pub const LMK_ASYNC_EVENT_STAT: i32 = 1;

/// `proc_type`.
pub const PROC_TYPE_APP: i32 = 0;
pub const PROC_TYPE_SERVICE: i32 = 1;

/// `kill_reasons::LOW_MEM`.
pub const KILL_REASON_LOW_MEM: i32 = 8;
/// `MAX_TASKNAME_LEN`.
const MAX_TASKNAME_LEN: usize = 128;

/// A packet's words.
pub fn decode(bytes: &[u8]) -> Vec<i32> {
    bytes
        .chunks_exact(4)
        .map(|w| i32::from_be_bytes(w.try_into().unwrap()))
        .collect()
}

pub fn encode(words: &[i32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_be_bytes()).collect()
}

/// `lmkd_pack_set_prockills`.
pub fn prockill(pid: i32, uid: u32, rss_kb: i64) -> Vec<u8> {
    encode(&[LMK_PROCKILL, pid, uid as i32, rss_kb as i32])
}

/// What an [`LMK_STAT_KILL_OCCURRED`] reports (`kill_stat`).
pub struct KillStat<'a> {
    pub uid: u32,
    pub taskname: &'a str,
    pub oom_score: i32,
    pub min_oom_score: i32,
    pub free_mem_kb: i64,
    pub rss_kb: i64,
}

/// `lmkd_pack_set_kill_occurred`, in the layout ActivityManager's
/// `LmkdStatsReporter` reads with `DataInputStream`. The memory-stat
/// fields other than RSS are -1 (not measured), as the original sends
/// when it has no stats for the process.
pub fn kill_occurred(st: &KillStat) -> Vec<u8> {
    let mut p = Vec::with_capacity(4 + 48 + 32 + 2 + MAX_TASKNAME_LEN);
    p.extend(LMK_STAT_KILL_OCCURRED.to_be_bytes());
    // pgfault, pgmajfault, rss_in_bytes, cache_in_bytes, swap_in_bytes,
    // process_start_time_ns.
    for v in [-1, -1, st.rss_kb * 1024, -1, -1, -1i64] {
        p.extend(v.to_be_bytes());
    }
    // uid, oom_score, min_oom_score, free_mem_kb, free_swap_kb,
    // kill_reason, thrashing, max_thrashing.
    for v in [
        st.uid as i32,
        st.oom_score,
        st.min_oom_score,
        st.free_mem_kb as i32,
        0,
        KILL_REASON_LOW_MEM,
        0,
        0,
    ] {
        p.extend(v.to_be_bytes());
    }
    // pack_string: a 16-bit length, the bytes and a NUL.
    let name = &st.taskname.as_bytes()[..st.taskname.len().min(MAX_TASKNAME_LEN - 1)];
    p.extend((name.len() as u16).to_be_bytes());
    p.extend(name);
    p.push(0);
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packets_are_big_endian_words() {
        assert_eq!(
            decode(&encode(&[LMK_PROCPRIO, 1234, 10001, 900, 0])),
            [1, 1234, 10001, 900, 0]
        );
        assert_eq!(prockill(7, 10001, 4096), encode(&[6, 7, 10001, 4096]));
        assert_eq!(CTRL_PACKET_MAX_SIZE, 52);
    }

    #[test]
    fn kill_occurred_matches_the_stats_layout() {
        let p = kill_occurred(&KillStat {
            uid: 10001,
            taskname: "com.example",
            oom_score: 950,
            min_oom_score: 900,
            free_mem_kb: 1000,
            rss_kb: 2,
        });
        assert_eq!(p.len(), 4 + 48 + 32 + 2 + 11 + 1);
        assert_eq!(&p[..4], &8i32.to_be_bytes());
        assert_eq!(&p[20..28], &2048i64.to_be_bytes());
        assert_eq!(&p[52..56], &10001i32.to_be_bytes());
        assert_eq!(&p[56..60], &950i32.to_be_bytes());
        assert_eq!(&p[72..76], &KILL_REASON_LOW_MEM.to_be_bytes());
        assert_eq!(&p[84..86], &11u16.to_be_bytes());
        assert_eq!(&p[86..97], b"com.example");
        assert_eq!(p[97], 0);
    }
}
