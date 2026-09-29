//! Fetching pinned AOSP sources from android.googlesource.com (gitiles)
//! into `_build/aosp`, each checked against the content hash of its lock.
//!
//! A tree is `<project without platform/>/<subtree>`; it is extracted once
//! (marked by `.fetched`) and verified on every run of its node. Archives
//! are kept in `_build/downloads`.

use crate::hash;
use crate::log::Log;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::Duration;

const GITILES: &str = "https://android.googlesource.com";
const ATTEMPTS: u64 = 6;

/// One fetched tree or file of a lock: `project|path|sha256[|revision]`.
pub struct Source {
    pub project: String,
    pub path: String,
    pub sha256: String,
    /// A tag (`refs/tags/...`) or an object id.
    pub revision: String,
}

impl Source {
    /// Parses a lock entry; `tag` is the lock's `AOSP_TAG`.
    pub fn parse(entry: &str, tag: &str) -> Result<Source, String> {
        let fields: Vec<&str> = entry.split('|').collect();
        let (project, path, sha256, revision) = match fields[..] {
            [project, path, sha256] => (project, path, sha256, None),
            [project, path, sha256, revision] => (project, path, sha256, Some(revision)),
            _ => return Err(format!("bad source entry `{entry}`")),
        };
        Ok(Source {
            project: project.into(),
            path: path.into(),
            sha256: sha256.into(),
            revision: revision.map_or(format!("refs/tags/{tag}"), String::from),
        })
    }

    /// Where it is extracted.
    pub fn dest(&self) -> PathBuf {
        let project = self
            .project
            .strip_prefix("platform/")
            .unwrap_or(&self.project);
        let dest = aim_paths::aosp().join(project);
        if self.path.is_empty() {
            dest
        } else {
            dest.join(&self.path)
        }
    }

    /// Fetches the tree (once) and checks its content hash.
    pub fn tree(&self, log: &mut Log) -> Result<PathBuf, String> {
        let dest = self.dest();
        self.extract(&dest, log)?;
        let got = hash::tree_hash(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        if got != self.sha256 {
            return Err(format!(
                "content hash mismatch: {}/{}: {got} (lock: {})",
                self.project, self.path, self.sha256
            ));
        }
        Ok(dest)
    }

    /// Fetches the tree into `dest` once, unverified (the caller checks it).
    pub fn extract(&self, dest: &Path, log: &mut Log) -> Result<(), String> {
        if dest.join(".fetched").exists() {
            return Ok(());
        }
        let name = format!("{}-{}", self.project, self.path).replace('/', "_");
        let archive = aim_paths::downloads().join(format!("{name}.tar.gz"));
        if dest.exists() {
            fs::remove_dir_all(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        }
        if !archive.exists() {
            let suffix = if self.path.is_empty() {
                self.revision.clone()
            } else {
                format!("{}/{}", self.revision, self.path)
            };
            let url = format!("{GITILES}/{}/+archive/{suffix}.tar.gz", self.project);
            let downloaded = gitiles(&url, &archive, log, |partial, log| {
                log.run(Command::new("tar").arg("-tzf").arg(partial))
                    .is_ok()
            });
            if let Err(error) = downloaded {
                // Gitiles refuses some archives for a long while; git's own
                // protocol serves the same tree.
                log.line(&format!("{error}; fetching with git instead"));
                self.git_extract(dest, log)?;
                return fs::write(dest.join(".fetched"), "").map_err(|e| e.to_string());
            }
        }
        fs::create_dir_all(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        log.run(
            Command::new("tar")
                .arg("-xzf")
                .arg(&archive)
                .arg("-C")
                .arg(dest),
        )?;
        fs::write(dest.join(".fetched"), "").map_err(|e| e.to_string())
    }

    /// Extracts the tree from a shallow, sparse git checkout.
    fn git_extract(&self, dest: &Path, log: &mut Log) -> Result<(), String> {
        self.git_checkout(dest, &format!("/{}/", self.path), log)
    }

    /// Checks `pattern` (a sparse-checkout pattern for `self.path`) out of
    /// a shallow, blob-less clone and moves `self.path` to `dest`.
    fn git_checkout(&self, dest: &Path, pattern: &str, log: &mut Log) -> Result<(), String> {
        let work = dest.with_extension("git-work");
        let _ = fs::remove_dir_all(&work);
        fs::create_dir_all(&work).map_err(|e| e.to_string())?;
        let url = format!("{GITILES}/{}", self.project);
        let git = |log: &mut Log, args: &[&str]| {
            log.run(Command::new("git").arg("-C").arg(&work).args(args))
        };
        git(log, &["init", "-q"])?;
        git(log, &["remote", "add", "origin", &url])?;
        git(
            log,
            &[
                "fetch",
                "-q",
                "--depth=1",
                "--filter=blob:none",
                "origin",
                &self.revision,
            ],
        )?;
        if !self.path.is_empty() {
            git(log, &["sparse-checkout", "set", "--no-cone", pattern])?;
        }
        git(
            log,
            &[
                "-c",
                "advice.detachedHead=false",
                "checkout",
                "-q",
                "FETCH_HEAD",
            ],
        )?;
        fs::remove_dir_all(work.join(".git")).map_err(|e| e.to_string())?;
        fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
        fs::rename(work.join(&self.path), dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        let _ = fs::remove_dir_all(&work);
        Ok(())
    }

    /// Fetches one file (once) and checks its sha256.
    pub fn file(&self, log: &mut Log) -> Result<PathBuf, String> {
        let dest = self.dest();
        if !dest.exists() {
            fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
            // Gitiles serves a file base64-encoded.
            let url = format!(
                "{GITILES}/{}/+/{}/{}?format=TEXT",
                self.project, self.revision, self.path
            );
            let encoded = dest.with_extension("base64");
            if let Err(error) = gitiles(&url, &encoded, log, |_, _| true) {
                log.line(&format!("{error}; fetching with git instead"));
                self.git_checkout(&dest, &format!("/{}", self.path), log)?;
                return self.check_file(&dest);
            }
            let partial = dest.with_extension("partial");
            log.run(
                Command::new("base64")
                    .arg("-D")
                    .arg("-i")
                    .arg(&encoded)
                    .arg("-o")
                    .arg(&partial),
            )?;
            fs::remove_file(&encoded).map_err(|e| e.to_string())?;
            fs::rename(&partial, &dest).map_err(|e| e.to_string())?;
        }
        self.check_file(&dest)
    }

    fn check_file(&self, dest: &Path) -> Result<PathBuf, String> {
        let got = hash::sha256_file(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        if got != self.sha256 {
            return Err(format!(
                "sha256 mismatch: {}/{}: {got} (lock: {})",
                self.project, self.path, self.sha256
            ));
        }
        Ok(dest.to_path_buf())
    }
}

/// Set once gitiles has refused a download in this run: the rest go
/// straight to git, rather than through every retry again.
static GITILES_REFUSED: AtomicBool = AtomicBool::new(false);

/// [`download`] from gitiles, unless it already refused one.
fn gitiles(
    url: &str,
    dest: &Path,
    log: &mut Log,
    valid: impl Fn(&Path, &mut Log) -> bool,
) -> Result<(), String> {
    if GITILES_REFUSED.load(Ordering::Relaxed) {
        return Err(format!(
            "not fetching {url}: gitiles refused an earlier download"
        ));
    }
    let result = download(url, dest, log, valid);
    if result.is_err() {
        GITILES_REFUSED.store(true, Ordering::Relaxed);
    }
    result
}

/// Downloads `url` to `dest`, retrying with a growing pause (gitiles
/// answers 503 when it throttles) until `valid` accepts the download.
pub fn download(
    url: &str,
    dest: &Path,
    log: &mut Log,
    valid: impl Fn(&Path, &mut Log) -> bool,
) -> Result<(), String> {
    fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
    let partial = dest.with_extension("partial");
    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            sleep(Duration::from_secs((5 * attempt).min(60)));
        }
        let code = log.output(
            Command::new("curl")
                .args(["-sL", "-w", "%{http_code}", "-o"])
                .arg(&partial)
                .arg(url),
        );
        if code.as_deref() == Ok("200") && valid(&partial, log) {
            return fs::rename(&partial, dest).map_err(|e| e.to_string());
        }
        log.line(&format!("fetch {url}: {code:?}, retrying"));
    }
    let _ = fs::remove_file(&partial);
    Err(format!("fetch failed: {url}"))
}
