//! `JobStore` at `android-16.0.0_r1`: the persisted jobs in
//! `/data/system/job`, one file per uid (`jobs_<uid>.xml`, the split files
//! of `DEFAULT_PERSIST_IN_SPLIT_FILES`) or the legacy `jobs.xml`, read as
//! `ReadJobMapFromDiskRunnable` reads them (each job rebuilt with
//! `JobInfo.Builder`, its wall-clock delay and deadline brought to the
//! elapsed clock) and written as `writeJobsMapImpl` writes them, in binary
//! XML through an atomic file.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use aim_android_xml::{Element, Node, Value};

use super::job_info::{
    self, CONSTRAINT_FLAG_BATTERY_NOT_LOW, CONSTRAINT_FLAG_CHARGING, CONSTRAINT_FLAG_DEVICE_IDLE,
    CONSTRAINT_FLAG_STORAGE_NOT_LOW, JobInfo, JobWorkItem, MAX_NUM_DEBUG_TAGS, NETWORK_BYTES_UNKNOWN,
};
use crate::package::restrictions::persistable::{self, Bundle};

/// `JobStatus.NO_EARLIEST_RUNTIME`, `NO_LATEST_RUNTIME`.
pub const NO_EARLIEST_RUNTIME: i64 = 0;
pub const NO_LATEST_RUNTIME: i64 = i64::MAX;
/// `JobStore.JOBS_FILE_VERSION`, `JOB_FILE_SPLIT_PREFIX`.
const JOBS_FILE_VERSION: i32 = 1;
const SPLIT_PREFIX: &str = "jobs_";
const LEGACY_FILE: &str = "jobs.xml";

/// A job as `JobStore` keeps it: the `JobStatus` fields it writes.
#[derive(Clone)]
pub struct Stored {
    pub job: JobInfo,
    /// The scheduling uid, whose file holds the job.
    pub uid: i32,
    pub source_package: Option<String>,
    pub source_user: i32,
    pub namespace: Option<String>,
    pub source_tag: Option<String>,
    pub internal_flags: i32,
    pub last_successful_run: i64,
    pub last_failed_run: i64,
    pub cumulative_execution: i64,
    /// Elapsed times, or `NO_EARLIEST_RUNTIME` and `NO_LATEST_RUNTIME`.
    pub earliest: i64,
    pub latest: i64,
    /// Executing work first, then pending.
    pub work: Vec<JobWorkItem>,
}

/// Each job file's name in `dir`, and whether it is a split file.
fn job_files(dir: &Path) -> Vec<(std::path::PathBuf, bool)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<_> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            let split = name
                .strip_prefix(SPLIT_PREFIX)
                .and_then(|n| n.strip_suffix(".xml"))
                .is_some_and(|uid| !uid.is_empty() && uid.bytes().all(|b| b.is_ascii_digit()));
            (split || name == LEGACY_FILE).then(|| (e.path(), split))
        })
        .collect();
    files.sort();
    files
}

/// `ReadJobMapFromDiskRunnable.run`: the jobs of every job file in `dir`,
/// at `now_elapsed` and `now_wall`; whether the legacy file was read (to be
/// migrated to split files). A job of the same uid, namespace and id as one
/// read before is dropped, as are jobs that do not read.
pub fn read(dir: &Path, now_elapsed: i64, now_wall: i64) -> (Vec<Stored>, bool) {
    let mut jobs: Vec<Stored> = Vec::new();
    let mut migrate = false;
    for (path, split) in job_files(dir) {
        migrate |= !split;
        let read = std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| read_file(&bytes, now_elapsed, now_wall));
        match read {
            Ok(found) => {
                for job in found {
                    let same = |j: &Stored| j.uid == job.uid && j.namespace == job.namespace && j.job.id == job.job.id;
                    if !jobs.iter().any(same) {
                        jobs.push(job);
                    }
                }
            }
            Err(e) => eprintln!("app host: jobs: error in {}: {e}", path.display()),
        }
    }
    (jobs, migrate)
}

/// `readJobMapImpl` of one file.
fn read_file(bytes: &[u8], now_elapsed: i64, now_wall: i64) -> Result<Vec<Stored>, String> {
    let Some(root) = aim_android_xml::read_next_optional(bytes)? else {
        return Ok(Vec::new());
    };
    if root.name != "job-info" {
        return Ok(Vec::new());
    }
    let version = root.int("version")?.ok_or("no version")?;
    if !(0..=JOBS_FILE_VERSION).contains(&version) {
        return Ok(Vec::new());
    }
    let mut jobs = Vec::new();
    for e in root.children().filter(|e| e.name == "job") {
        match restore_job(e, version, now_elapsed, now_wall) {
            Ok(job) => jobs.push(job),
            Err(why) => eprintln!("app host: jobs: a job not read: {why}"),
        }
    }
    Ok(jobs)
}

fn required<T>(value: Result<Option<T>, String>, name: &str) -> Result<T, String> {
    value?.ok_or_else(|| format!("no {name}"))
}

/// A comma-separated list of ints (`JobStore.stringToIntArray`).
fn int_list(s: &str) -> Result<Vec<i32>, String> {
    if s.is_empty() {
        return Ok(Vec::new());
    }
    s.split(',').map(|v| v.parse().map_err(|_| format!("not a number: {v}"))).collect()
}

/// `restoreJobFromXml`.
fn restore_job(e: &Element, version: i32, now_elapsed: i64, now_wall: i64) -> Result<Stored, String> {
    let id = required(e.int("jobid"), "jobid")?;
    let package = e.string("package").ok_or("no package")?.into_owned();
    let class = e.string("class").ok_or("no class")?.into_owned();
    let mut job = JobInfo::persisted_builder(id, package, class);
    let uid = required(e.int("uid"), "uid")?;
    if version == 0 {
        job.bias = e.int("priority")?.unwrap_or(job.bias);
    } else {
        job.bias = e.int("bias")?.unwrap_or(job.bias);
        job.priority = e.int("priority")?.unwrap_or(job.priority);
    }
    job.flags = e.int("flags")?.unwrap_or(0);
    let internal_flags = e.int("internalFlags")?.unwrap_or(0);
    let source_user = e.int("sourceUserId")?.unwrap_or(-1);
    let last_successful_run = e.long("lastSuccessfulRunTime")?.unwrap_or(0);
    let last_failed_run = e.long("lastFailedRunTime")?.unwrap_or(0);
    let cumulative_execution = e.long("cumulativeExecutionTime")?.unwrap_or(0);
    let mut source_package = e.string("sourcePackageName").map(|s| s.into_owned());
    let namespace = e.string("namespace").map(|s| s.into_owned());
    let source_tag = e.string("sourceTag").map(|s| s.into_owned());

    let mut children = e.children();
    let constraints = children.next().filter(|c| c.name == "constraints").ok_or("no constraints")?;
    read_constraints(&mut job, constraints)?;
    let criteria = children.next().ok_or("no execution criteria")?;
    // `buildRtcExecutionTimesFromXml`, then `convertRtcBoundsToElapsed`.
    let delay = criteria.long("delay")?.unwrap_or(NO_EARLIEST_RUNTIME);
    let deadline = criteria.long("deadline")?.unwrap_or(NO_LATEST_RUNTIME);
    let mut earliest = if delay > NO_EARLIEST_RUNTIME {
        now_elapsed + (delay - now_wall).max(0)
    } else {
        NO_EARLIEST_RUNTIME
    };
    let mut latest = if deadline < NO_LATEST_RUNTIME {
        now_elapsed + (deadline - now_wall).max(0)
    } else {
        NO_LATEST_RUNTIME
    };
    match criteria.name.as_str() {
        "periodic" => {
            let period = required(criteria.long("period"), "period")?;
            let flex = criteria.long("flex")?.unwrap_or(period);
            job.set_periodic(period, flex);
            // Clamped to a window no later than a period and flex from now.
            if latest > now_elapsed.saturating_add(period).saturating_add(flex) {
                latest = now_elapsed + flex + period;
                earliest = latest - flex;
            }
        }
        "one-off" => {
            if earliest != NO_EARLIEST_RUNTIME {
                job.min_latency = earliest - now_elapsed;
                job.has_early_constraint = true;
            }
            if latest != NO_LATEST_RUNTIME {
                job.max_execution_delay = latest - now_elapsed;
                job.has_late_constraint = true;
            }
        }
        other => return Err(format!("invalid parameter tag {other}")),
    }
    // `maybeBuildBackoffPolicyFromXml`.
    let backoff_set = match criteria.long("initial-backoff")? {
        Some(initial) => {
            job.set_backoff(initial, required(criteria.int("backoff-policy"), "backoff-policy")?);
            true
        }
        None => false,
    };
    let extras = children.next().filter(|c| c.name == "extras").ok_or("no extras")?;
    job.extras = Bundle::restore(extras).map_err(|e| e.to_string())?;
    let mut work = Vec::new();
    let mut next = children.next();
    while let Some(item) = next.filter(|c| c.name == "job-work-item") {
        match read_work_item(item) {
            Ok(w) => work.push(w),
            Err(why) => eprintln!("app host: jobs: a persisted work item not read: {why}"),
        }
        next = children.next();
    }
    if let Some(info) = next.filter(|c| c.name == "debug-info") {
        job.trace_tag = info.string("trace-tag").map(|s| s.into_owned());
        for tag in info.children().filter(|c| c.name == "debug-tag") {
            if job.debug_tags.len() >= MAX_NUM_DEBUG_TAGS {
                break;
            }
            if let Some(Ok(tag)) = tag.string("tag").map(|t| job_info::validate_debug_tag(&t))
                && !job.debug_tags.contains(&tag)
            {
                job.debug_tags.push(tag);
            }
        }
    }
    // `Builder.build(false, false, false, false)`.
    if backoff_set && job.constraint_flags & CONSTRAINT_FLAG_DEVICE_IDLE != 0 {
        return Err("An idle mode job will not respect any back-off policy".into());
    }
    job.enforce_validity(false, false, false, false).map_err(|e| e.message)?;
    // Sync jobs of an earlier, incomplete representation.
    if source_package.as_deref() == Some("android") {
        let entry = |k: &str| job.extras.entries.iter().find(|(n, _)| n.as_deref() == Some(k)).map(|(_, v)| v);
        if matches!(entry("SyncManagerJob"), Some(persistable::Value::Bool(true)))
            && let Some(persistable::Value::String(owner)) = entry("owningPackage")
        {
            source_package = Some(owner.clone());
        }
    }
    Ok(Stored {
        job,
        uid,
        source_package,
        source_user,
        namespace,
        source_tag,
        internal_flags,
        last_successful_run,
        last_failed_run,
        cumulative_execution,
        earliest,
        latest,
        work,
    })
}

/// `buildConstraintsFromXml`, of its S+ form.
fn read_constraints(job: &mut JobInfo, e: &Element) -> Result<(), String> {
    let caps = e.string("net-capabilities-csv");
    let transports = e.string("net-transport-types-csv");
    if let (Some(caps), Some(transports)) = (&caps, &transports) {
        let forbidden = e.string("net-forbidden-capabilities-csv").unwrap_or_default();
        job.network = Some(job_info::restored_request(&int_list(caps)?, &int_list(&forbidden)?, &int_list(transports)?));
        job.network_download_bytes = e.long("estimated-download-bytes")?.unwrap_or(NETWORK_BYTES_UNKNOWN);
        job.network_upload_bytes = e.long("estimated-upload-bytes")?.unwrap_or(NETWORK_BYTES_UNKNOWN);
        let chunk = e.long("minimum-network-chunk-bytes")?.unwrap_or(NETWORK_BYTES_UNKNOWN);
        if chunk != NETWORK_BYTES_UNKNOWN && chunk <= 0 {
            return Err("Minimum chunk size must be positive".into());
        }
        job.minimum_network_chunk_bytes = chunk;
    } else if ["net-capabilities", "connectivity", "metered", "unmetered", "not-roaming"]
        .iter()
        .any(|a| e.attr(a).is_some())
    {
        // Written by R and before; this image's JobStore wrote S's form.
        return Err("a network constraint of R or before".into());
    }
    for (name, flag) in [
        ("idle", CONSTRAINT_FLAG_DEVICE_IDLE),
        ("charging", CONSTRAINT_FLAG_CHARGING),
        ("battery-not-low", CONSTRAINT_FLAG_BATTERY_NOT_LOW),
        ("storage-not-low", CONSTRAINT_FLAG_STORAGE_NOT_LOW),
    ] {
        if e.attr(name).is_some() {
            job.constraint_flags |= flag;
        }
    }
    Ok(())
}

/// `readJobWorkItemFromXml`.
fn read_work_item(e: &Element) -> Result<JobWorkItem, String> {
    let delivery_count = required(e.int("delivery-count"), "delivery-count")?;
    let long = |name: &str| e.long(name).map(|v| v.unwrap_or(NETWORK_BYTES_UNKNOWN));
    let (down, up, chunk) =
        (long("estimated-download-bytes")?, long("estimated-upload-bytes")?, long("minimum-network-chunk-bytes")?);
    let extras = match e.children().next() {
        Some(extras) => Bundle::restore(extras).map_err(|e| e.to_string())?,
        None => Bundle::default(),
    };
    let item = JobWorkItem::persisted(extras, delivery_count, down, up, chunk);
    item.enforce_validity(true).map_err(|e| e.message)?;
    Ok(item)
}

fn string(name: &str, value: impl ToString) -> (String, Value) {
    (name.into(), Value::String(value.to_string()))
}

fn element(name: &str, attrs: Vec<(String, Value)>, content: Vec<Node>) -> Node {
    Node::Element(Element { name: name.into(), attrs, content })
}

/// `writeBundleToXml`.
fn extras(b: &Bundle) -> Result<Node, String> {
    b.save("extras").map(Node::Element).map_err(|e| e.to_string())
}

/// `writeJobsMapImpl`'s `<job>` of `s`, at `now_elapsed` and `now_wall`.
fn job_element(s: &Stored, now_elapsed: i64, now_wall: i64) -> Result<Node, String> {
    let job = &s.job;
    // `addAttributesToJobTag`.
    let mut attrs = vec![string("jobid", job.id), string("package", &job.service.0), string("class", &job.service.1)];
    if let Some(p) = &s.source_package {
        attrs.push(string("sourcePackageName", p));
    }
    if let Some(n) = &s.namespace {
        attrs.push(string("namespace", n));
    }
    if let Some(t) = &s.source_tag {
        attrs.push(string("sourceTag", t));
    }
    attrs.extend([
        string("sourceUserId", s.source_user),
        string("uid", s.uid),
        string("bias", job.bias),
        string("priority", job.priority),
        string("flags", job.flags),
    ]);
    if s.internal_flags != 0 {
        attrs.push(string("internalFlags", s.internal_flags));
    }
    attrs.extend([
        string("lastSuccessfulRunTime", s.last_successful_run),
        string("lastFailedRunTime", s.last_failed_run),
        ("cumulativeExecutionTime".into(), Value::Long(s.cumulative_execution)),
    ]);
    // `writeConstraintsToXml`.
    let mut constraints = Vec::new();
    if let Some(n) = &job.network {
        let csv = |bits: u64| job_info::unpack_bits(bits).iter().map(i32::to_string).collect::<Vec<_>>().join(",");
        constraints.extend([
            string("net-capabilities-csv", csv(n.caps.capabilities)),
            string("net-forbidden-capabilities-csv", csv(n.caps.forbidden)),
            string("net-transport-types-csv", csv(n.caps.transports)),
        ]);
        for (name, v) in [
            ("estimated-download-bytes", job.network_download_bytes),
            ("estimated-upload-bytes", job.network_upload_bytes),
            ("minimum-network-chunk-bytes", job.minimum_network_chunk_bytes),
        ] {
            if v != NETWORK_BYTES_UNKNOWN {
                constraints.push((name.into(), Value::Long(v)));
            }
        }
    }
    for (name, flag) in [
        ("idle", CONSTRAINT_FLAG_DEVICE_IDLE),
        ("charging", CONSTRAINT_FLAG_CHARGING),
        ("battery-not-low", CONSTRAINT_FLAG_BATTERY_NOT_LOW),
        ("storage-not-low", CONSTRAINT_FLAG_STORAGE_NOT_LOW),
    ] {
        if job.constraint_flags & flag != 0 {
            constraints.push(string(name, true));
        }
    }
    // `writeExecutionCriteriaToXml`: the delay and deadline on the wall
    // clock.
    let mut criteria = Vec::new();
    if job.periodic {
        criteria.extend([string("period", job.interval), string("flex", job.flex)]);
    }
    if s.latest != NO_LATEST_RUNTIME {
        criteria.push(string("deadline", now_wall + (s.latest - now_elapsed)));
    }
    if s.earliest != NO_EARLIEST_RUNTIME {
        criteria.push(string("delay", now_wall + (s.earliest - now_elapsed)));
    }
    if job.initial_backoff != job_info::DEFAULT_INITIAL_BACKOFF_MILLIS
        || job.backoff_policy != job_info::BACKOFF_POLICY_EXPONENTIAL
    {
        criteria.extend([string("backoff-policy", job.backoff_policy), string("initial-backoff", job.initial_backoff)]);
    }
    let mut content = vec![
        element("constraints", constraints, Vec::new()),
        element(if job.periodic { "periodic" } else { "one-off" }, criteria, Vec::new()),
        extras(&job.extras)?,
    ];
    // `writeJobWorkItemsToXml`: items with an intent are not persisted.
    for w in s.work.iter().filter(|w| w.intent.is_none()) {
        let mut attrs = vec![("delivery-count".into(), Value::Int(w.delivery_count))];
        for (name, v) in [
            ("estimated-download-bytes", w.network_download_bytes),
            ("estimated-upload-bytes", w.network_upload_bytes),
            ("minimum-network-chunk-bytes", w.minimum_chunk_bytes),
        ] {
            if v != NETWORK_BYTES_UNKNOWN {
                attrs.push((name.into(), Value::Long(v)));
            }
        }
        content.push(element("job-work-item", attrs, vec![extras(&w.extras)?]));
    }
    // `writeDebugInfoToXml`.
    if job.trace_tag.is_some() || !job.debug_tags.is_empty() {
        let attrs = job.trace_tag.iter().map(|t| string("trace-tag", t)).collect();
        let tags = job.debug_tags.iter().map(|t| element("debug-tag", vec![string("tag", t)], Vec::new())).collect();
        content.push(element("debug-info", attrs, tags));
    }
    Ok(element("job", attrs, content))
}

/// The document `writeJobsMapImpl` writes of `jobs`.
fn document(jobs: &[&Stored], now_elapsed: i64, now_wall: i64) -> Result<Element, String> {
    Ok(Element {
        name: "job-info".into(),
        attrs: vec![string("version", JOBS_FILE_VERSION)],
        content: jobs.iter().map(|s| job_element(s, now_elapsed, now_wall)).collect::<Result<_, _>>()?,
    })
}

/// `mWriteRunnable` for `uid`: its persisted jobs written to its split
/// file, as `AtomicFile` writes it.
pub fn write(dir: &Path, uid: i32, jobs: &[&Stored], now_elapsed: i64, now_wall: i64) -> std::io::Result<()> {
    let bytes = aim_android_xml::abx::write(&document(jobs, now_elapsed, now_wall).map_err(std::io::Error::other)?)
        .map_err(std::io::Error::other)?;
    if !dir.is_dir() {
        std::fs::create_dir_all(dir)?;
        aim_storage::guest_inode::record(
            dir,
            aim_storage::guest_inode::GuestInode { uid: Some(1000), gid: Some(1000), mode: Some(0o700) },
        )?;
    }
    let path = dir.join(format!("{SPLIT_PREFIX}{uid}.xml"));
    let new = dir.join(format!("{SPLIT_PREFIX}{uid}.xml.new"));
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&new)?;
    std::io::Write::write_all(&mut file, &bytes)?;
    crate::fsync(&file)?;
    aim_storage::guest_inode::record(
        &new,
        aim_storage::guest_inode::GuestInode { uid: Some(1000), gid: Some(1000), mode: Some(0o600) },
    )?;
    std::fs::rename(&new, path)
}

/// The legacy file, once its jobs are in split files.
pub fn remove_legacy(dir: &Path) {
    if let Err(e) = std::fs::remove_file(dir.join(LEGACY_FILE))
        && e.kind() != std::io::ErrorKind::NotFound
    {
        eprintln!("app host: jobs: removing {LEGACY_FILE}: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::restrictions::persistable::Value as P;

    const NOW_ELAPSED: i64 = 1_000_000;
    const NOW_WALL: i64 = 1_760_000_000_000;

    /// A periodic network job and a one-off job with a work item, as
    /// `writeJobsMapImpl` wrote them (in text XML, as `jobs.xml` was).
    const JOBS: &str = r#"<?xml version='1.0' encoding='utf-8' standalone='yes' ?>
<job-info version="1">
<job jobid="42" package="com.example" class="com.example.Sync" sourcePackageName="com.example" sourceUserId="0" uid="10123" bias="0" priority="300" flags="0" lastSuccessfulRunTime="0" lastFailedRunTime="0" cumulativeExecutionTime="0">
<constraints net-capabilities-csv="12,16" net-forbidden-capabilities-csv="" net-transport-types-csv="" charging="true" />
<periodic period="3600000" flex="3600000" deadline="1760003600000" delay="1760000000000" />
<extras>
<int name="count" value="3" />
</extras>
</job>
<job jobid="7" package="com.example" class="com.example.Index" sourcePackageName="com.example" namespace="index" sourceUserId="0" uid="10123" bias="0" priority="300" flags="0" lastSuccessfulRunTime="5" lastFailedRunTime="6" cumulativeExecutionTime="7">
<constraints storage-not-low="true" />
<one-off delay="1760000060000" backoff-policy="0" initial-backoff="20000" />
<extras />
<job-work-item delivery-count="1">
<extras>
<string name="what">all</string>
</extras>
</job-work-item>
<debug-info trace-tag="index">
<debug-tag tag="a" />
</debug-info>
</job>
</job-info>
"#;

    #[test]
    fn persisted_jobs_read_as_the_original_reads_them() {
        let jobs = read_file(JOBS.as_bytes(), NOW_ELAPSED, NOW_WALL).unwrap();
        assert_eq!(jobs.len(), 2);
        let sync = &jobs[0];
        assert_eq!((sync.uid, sync.job.id, sync.job.periodic, sync.job.persisted), (10123, 42, true, true));
        assert_eq!((sync.earliest, sync.latest), (NOW_ELAPSED, NOW_ELAPSED + 3_600_000));
        assert_eq!(sync.job.constraint_flags, CONSTRAINT_FLAG_CHARGING);
        assert_eq!(job_info::unpack_bits(sync.job.network.as_ref().unwrap().caps.capabilities), [12, 16]);
        assert_eq!(sync.job.extras.entries, vec![(Some("count".into()), P::Int(3))]);
        let index = &jobs[1];
        assert_eq!(index.namespace.as_deref(), Some("index"));
        assert_eq!((index.earliest, index.latest), (NOW_ELAPSED + 60_000, NO_LATEST_RUNTIME));
        assert_eq!((index.job.min_latency, index.job.has_early_constraint), (60_000, true));
        assert_eq!((index.job.backoff_policy, index.job.initial_backoff), (job_info::BACKOFF_POLICY_LINEAR, 20_000));
        assert_eq!((index.last_successful_run, index.last_failed_run, index.cumulative_execution), (5, 6, 7));
        assert_eq!(index.work.len(), 1);
        assert_eq!(index.work[0].extras.entries, vec![(Some("what".into()), P::String("all".into()))]);
        assert_eq!((index.job.trace_tag.as_deref(), index.job.debug_tags.as_slice()), (Some("index"), &["a".to_string()][..]));
    }

    /// What is written reads back the same, in binary XML, and its job
    /// tags carry the original's attributes in the original's order.
    #[test]
    fn persisted_jobs_round_trip() {
        let jobs = read_file(JOBS.as_bytes(), NOW_ELAPSED, NOW_WALL).unwrap();
        let refs: Vec<&Stored> = jobs.iter().collect();
        let doc = document(&refs, NOW_ELAPSED, NOW_WALL).unwrap();
        let first = doc.children().next().unwrap();
        let names: Vec<&str> = first.attrs.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "jobid", "package", "class", "sourcePackageName", "sourceUserId", "uid", "bias", "priority", "flags",
                "lastSuccessfulRunTime", "lastFailedRunTime", "cumulativeExecutionTime"
            ]
        );
        let criteria = first.children().nth(1).unwrap();
        assert_eq!(criteria.string("deadline").as_deref(), Some("1760003600000"));
        let bytes = aim_android_xml::abx::write(&doc).unwrap();
        let back = read_file(&bytes, NOW_ELAPSED, NOW_WALL).unwrap();
        assert_eq!(back.len(), 2);
        for (a, b) in jobs.iter().zip(&back) {
            assert!(a.job.same_as(&b.job));
            assert_eq!((a.earliest, a.latest, a.work.len()), (b.earliest, b.latest, b.work.len()));
        }
    }

    #[test]
    fn a_job_of_an_unreadable_constraint_is_dropped() {
        let xml = r#"<job-info version="1"><job jobid="1" package="p" class="p.S" uid="10001"><constraints connectivity="true" /><one-off /><extras /></job></job-info>"#;
        assert!(read_file(xml.as_bytes(), NOW_ELAPSED, NOW_WALL).unwrap().is_empty());
        let newer = r#"<job-info version="2" />"#;
        assert!(read_file(newer.as_bytes(), NOW_ELAPSED, NOW_WALL).unwrap().is_empty());
    }
}
