//! android-16.0.0_r1 JobStore/AtomicFile crash and write-batch contracts.
use super::*;
use job_store::{WriteStep as Step, with_io_hook};
use std::{io, time::Duration};

fn fail() -> io::Error {
    io::Error::other("injected JobStore I/O failure")
}
fn dir(host: &AppHost) -> std::path::PathBuf {
    host.inputs().unwrap().data.join("system/job")
}
fn flush(jobs: &JobScheduler) -> io::Result<()> {
    let mut state = jobs.state.lock().unwrap();
    let writes = JobScheduler::pending_writes(&mut state);
    jobs.write_jobs(&mut state, writes, elapsed_ms())
}
fn disk(host: &AppHost) -> Vec<Stored> {
    job_store::read(&dir(host), elapsed_ms(), wall_ms()).0
}
fn seed(host: &Arc<AppHost>, legacy: bool) {
    let jobs = scheduler(host);
    for (ns, package, tag) in [
        (None, "p", "default"),
        (Some("SyncManager"), "q", "old"),
        (Some("other"), "p", "other"),
    ] {
        schedule_sync(&jobs, ns, Some((package, 0, tag)));
    }
    flush(&jobs).unwrap();
    if legacy {
        std::fs::rename(dir(host).join("jobs_1000.xml"), dir(host).join("jobs.xml")).unwrap();
    }
}
fn changed(jobs: &JobScheduler) {
    cancel_sync(jobs, None);
    schedule_sync(jobs, Some("SyncManager"), Some(("p", 0, "new")));
    schedule_sync(jobs, Some("new"), Some(("q", 0, "added")));
}
fn assert_corpus(jobs: &JobScheduler, updated: bool) {
    assert_source(jobs, Some("other"), "p", 10100, Some("other"));
    if updated {
        assert_source(jobs, Some("SyncManager"), "p", 10100, Some("new"));
        assert_source(jobs, Some("new"), "q", 10101, Some("added"));
    } else {
        assert_source(jobs, None, "p", 10100, Some("default"));
        assert_source(jobs, Some("SyncManager"), "q", 10101, Some("old"));
    }
    let state = jobs.state.lock().unwrap();
    assert_eq!(state.jobs.len(), 3);
    assert_eq!(
        JobScheduler::find(&state, 1000, None, 42).is_none(),
        updated
    );
}

#[test]
fn atomic_file_restart_selection_matches_jobstore_file_filter() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        install_sync_manifest(host);
        for legacy in [false, true] {
            seed(host, legacy);
            let base = dir(host).join(if legacy { "jobs.xml" } else { "jobs_1000.xml" });
            let original = std::fs::read(&base).unwrap();
            let backup = base.with_extension("xml.bak");
            let new = base.with_extension("xml.new");
            std::fs::write(&backup, &original).unwrap();
            std::fs::write(&base, b"corrupt main").unwrap();
            std::fs::write(&new, b"ABX\0partial").unwrap();
            assert_corpus(&scheduler(host), false);
            assert_eq!(std::fs::read(&base).unwrap(), original);
            assert!(!backup.exists() && !new.exists());
            // JobStore filters directory entries before AtomicFile.openRead: an
            // orphan .bak or .new is not independently a job file.
            std::fs::rename(&base, &backup).unwrap();
            std::fs::write(&new, &original).unwrap();
            assert!(disk(host).is_empty());
            assert!(backup.exists() && new.exists());
            // startWrite, like openRead, restores a backup when a base is selected.
            let stored = status(sync_job(), 1000, 10100, "p", Some("SyncManager")).stored();
            with_io_hook(
                Box::new(|_, step| {
                    if step == Step::Open {
                        Err(fail())
                    } else {
                        Ok(())
                    }
                }),
                || {
                    assert!(
                        job_store::write(&dir(host), 1000, &[&stored], 1, 2, &mut [0; 3]).is_err()
                    );
                },
            );
            let restored = dir(host).join("jobs_1000.xml");
            if !legacy {
                assert_eq!(std::fs::read(restored).unwrap(), original);
                assert!(!backup.exists());
            }
            std::fs::remove_dir_all(dir(host)).unwrap();
        }
    });
}

#[test]
fn split_write_faults_retry_dirty_state_and_preserve_restart_identity() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        install_sync_manifest(host);
        for step in [
            Step::Open,
            Step::Job,
            Step::Data,
            Step::FileSync,
            Step::Rename,
            Step::DirectorySync,
        ] {
            seed(host, false);
            let jobs = scheduler(host);
            jobs.persist_stats().unwrap();
            changed(&jobs);
            with_io_hook(
                Box::new(move |path, at| {
                    if at == step {
                        if at == Step::Data {
                            std::fs::write(path.with_extension("xml.new"), b"ABX\0partial")?;
                        }
                        return Err(fail());
                    }
                    Ok(())
                }),
                || assert!(jobs.jobs_of(1000).is_err()),
            );
            let expected = if matches!(step, Step::Open | Step::Job) {
                0
            } else {
                3
            };
            assert_eq!(
                jobs.persist_stats().unwrap(),
                [3, 3, 3, expected, expected, expected]
            );
            assert_corpus(&scheduler(host), step == Step::DirectorySync);
            let mut state = jobs.state.lock().unwrap();
            assert_eq!(state.dirty, BTreeSet::from([1000]));
            let due = state.write_at.unwrap();
            assert!(jobs.tick(&mut state, due - 1).1.is_empty());
            let (_, writes) = jobs.tick(&mut state, due);
            assert_eq!(writes.len(), 1);
            jobs.write_jobs(&mut state, writes, due).unwrap();
            assert!(state.dirty.is_empty() && state.write_at.is_none());
            assert_eq!(state.persist_stats.0, [3, 3, 3, 3, 3, 3]);
            drop(state);
            assert_corpus(&scheduler(host), true);
            std::fs::remove_dir_all(dir(host)).unwrap();
        }
    });
}

#[test]
fn legacy_migration_faults_keep_source_until_split_commit_and_retry_unlink_sync() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        install_sync_manifest(host);
        for step in [
            Step::Open,
            Step::Job,
            Step::Data,
            Step::FileSync,
            Step::Rename,
            Step::DirectorySync,
            Step::LegacyRemove,
            Step::LegacySync,
        ] {
            seed(host, true);
            let jobs = scheduler(host);
            assert_eq!(jobs.persist_stats().unwrap(), [3, 3, 3, -1, -1, -1]);
            changed(&jobs);
            with_io_hook(
                Box::new(move |_, at| if at == step { Err(fail()) } else { Ok(()) }),
                || {
                    assert!(jobs.system_scheduled_own_jobs(Some("SyncManager")).is_err());
                },
            );
            assert_eq!(
                dir(host).join("jobs.xml").exists(),
                step != Step::LegacySync
            );
            assert!(jobs.state.lock().unwrap().migrate);
            let reopened = scheduler(host);
            if step == Step::LegacySync {
                assert_corpus(&reopened, true);
            } else {
                // First duplicate wins, as in ReadJobMapFromDiskRunnable.
                // A partially migrated directory can merge distinct namespaces;
                // it is not a transaction spanning every job file.
                assert_source(&reopened, None, "p", 10100, Some("default"));
                assert_source(&reopened, Some("SyncManager"), "q", 10101, Some("old"));
                let committed = matches!(step, Step::DirectorySync | Step::LegacyRemove);
                if committed {
                    assert_source(&reopened, Some("new"), "q", 10101, Some("added"));
                }
                assert_eq!(
                    reopened.state.lock().unwrap().jobs.len(),
                    if committed { 4 } else { 3 }
                );
            }
            // If unlink succeeded but its directory sync failed, retry must
            // still sync the directory when jobs.xml is already absent.
            with_io_hook(
                Box::new(|_, at| {
                    if at == Step::LegacySync {
                        Err(fail())
                    } else {
                        Ok(())
                    }
                }),
                || {
                    assert!(flush(&jobs).is_err());
                },
            );
            flush(&jobs).unwrap();
            assert!(!jobs.state.lock().unwrap().migrate);
            assert!(!dir(host).join("jobs.xml").exists());
            assert_corpus(&scheduler(host), true);
            std::fs::remove_dir_all(dir(host)).unwrap();
        }
    });
}

#[test]
fn failed_batch_counts_serialized_jobs_and_continues_other_uids() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        install_sync_manifest(host);
        let jobs = scheduler(host);
        jobs.persist_stats().unwrap();
        let mut state = jobs.state.lock().unwrap();
        let sync = status(sync_job(), 1000, 10100, "p", Some("SyncManager")).stored();
        let mut own = sync.clone();
        own.job.id = 43;
        own.job.service.1 = "android.Other".into();
        let mut app = sync.clone();
        app.uid = 10100;
        let writes = vec![(1000, vec![sync, own]), (10100, vec![app])];
        // Descending UID order as in mWriteRunnable. A failed startWrite
        // contributes zero; it must not prevent writing the next UID.
        with_io_hook(
            Box::new(|path, at| {
                if path.ends_with("jobs_10100.xml") && at == Step::Open {
                    Err(fail())
                } else {
                    Ok(())
                }
            }),
            || assert!(jobs.write_jobs(&mut state, writes.clone(), 100).is_err()),
        );
        assert_eq!(state.persist_stats.0, [0, 0, 0, 2, 2, 1]);
        assert_eq!(disk(host).len(), 2);
        let mut serial = 0;
        with_io_hook(
            Box::new(move |path, at| {
                if path.ends_with("jobs_1000.xml") && at == Step::Job {
                    serial += 1;
                    if serial == 2 {
                        return Err(fail());
                    }
                }
                Ok(())
            }),
            || assert!(jobs.write_jobs(&mut state, writes.clone(), 200).is_err()),
        );
        assert_eq!(state.persist_stats.0, [0, 0, 0, 2, 1, 1]);
        assert_eq!(state.dirty, BTreeSet::from([1000, 10100]));
        // A real ABX serialization error in the second job must likewise
        // count only completed job end tags, not the whole prepared document.
        let mut invalid = writes.clone();
        invalid[0].1[1].source_tag = Some("x".repeat(65536));
        assert!(jobs.write_jobs(&mut state, invalid, 250).is_err());
        assert_eq!(state.persist_stats.0, [0, 0, 0, 2, 1, 1]);
        // A clean no-op must retain the last attempted batch counters.
        jobs.write_jobs(&mut state, Vec::new(), 300).unwrap();
        assert_eq!(state.persist_stats.0, [0, 0, 0, 2, 1, 1]);
        jobs.write_jobs(&mut state, writes, 400).unwrap();
        assert_eq!(state.persist_stats.0, [0, 0, 0, 3, 2, 1]);
    });
}

#[test]
fn concurrent_updates_wait_for_write_then_persist_newest_namespace_snapshot() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        install_sync_manifest(host);
        for failed in [false, true] {
            seed(host, false);
            let jobs = scheduler(host);
            jobs.persist_stats().unwrap();
            let (entered, writing) = mpsc::channel();
            let (resume, proceed) = mpsc::channel();
            let writer_jobs = jobs.clone();
            let writer = std::thread::spawn(move || {
                with_io_hook(
                    Box::new(move |_, at| {
                        if at == Step::Rename {
                            entered.send(()).unwrap();
                            proceed.recv_timeout(Duration::from_secs(10)).unwrap();
                            if failed {
                                return Err(fail());
                            }
                        }
                        Ok(())
                    }),
                    || assert_eq!(flush(&writer_jobs).is_err(), failed),
                );
            });
            writing.recv_timeout(Duration::from_secs(10)).unwrap();
            assert!(jobs.state.try_lock().is_err());
            let (started, starting) = mpsc::channel();
            let (done, finished) = mpsc::channel();
            let update_jobs = jobs.clone();
            let update = std::thread::spawn(move || {
                started.send(()).unwrap();
                changed(&update_jobs);
                done.send(()).unwrap();
            });
            starting.recv_timeout(Duration::from_secs(10)).unwrap();
            assert!(finished.recv_timeout(Duration::from_millis(20)).is_err());
            resume.send(()).unwrap();
            writer.join().unwrap();
            update.join().unwrap();
            assert_corpus(&scheduler(host), false);
            assert_eq!(jobs.state.lock().unwrap().dirty, BTreeSet::from([1000]));
            jobs.jobs_of(1000).unwrap();
            assert_corpus(&scheduler(host), true);
            std::fs::remove_dir_all(dir(host)).unwrap();
        }
    });
}

#[test]
fn migration_retains_legacy_after_one_uid_fails_and_syncs_empty_store_removal() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        install_sync_manifest(host);
        let sync = status(sync_job(), 1000, 10100, "p", Some("SyncManager")).stored();
        let mut app = sync.clone();
        app.uid = 10100;
        app.namespace = None;
        job_store::write(
            &dir(host),
            1000,
            &[&sync, &app],
            elapsed_ms(),
            wall_ms(),
            &mut [0; 3],
        )
        .unwrap();
        std::fs::rename(dir(host).join("jobs_1000.xml"), dir(host).join("jobs.xml")).unwrap();
        let jobs = scheduler(host);
        assert_eq!(jobs.persist_stats().unwrap(), [2, 1, 1, -1, -1, -1]);
        with_io_hook(
            Box::new(|path, at| {
                if path.ends_with("jobs_1000.xml") && at == Step::Open {
                    Err(fail())
                } else {
                    Ok(())
                }
            }),
            || assert!(flush(&jobs).is_err()),
        );
        assert!(dir(host).join("jobs.xml").exists());
        assert!(dir(host).join("jobs_10100.xml").exists());
        assert_eq!(jobs.persist_stats().unwrap(), [2, 1, 1, 1, 0, 0]);
        assert_eq!(disk(host).len(), 2); // duplicate app identity is dropped
        flush(&jobs).unwrap();
        assert!(!dir(host).join("jobs.xml").exists());
        assert_eq!(jobs.persist_stats().unwrap(), [2, 1, 1, 2, 1, 1]);
        assert_eq!(disk(host).len(), 2);
        std::fs::remove_dir_all(dir(host)).unwrap();
        std::fs::create_dir_all(dir(host)).unwrap();
        std::fs::write(dir(host).join("jobs.xml"), b"<job-info version=\"1\" />").unwrap();
        let empty = scheduler(host);
        assert_eq!(empty.persist_stats().unwrap(), [0, 0, 0, -1, -1, -1]);
        with_io_hook(
            Box::new(|_, at| {
                if at == Step::LegacySync {
                    Err(fail())
                } else {
                    Ok(())
                }
            }),
            || {
                assert!(flush(&empty).is_err());
            },
        );
        assert!(empty.state.lock().unwrap().migrate);
        assert_eq!(empty.persist_stats().unwrap(), [0; 6]);
        flush(&empty).unwrap();
        assert!(!empty.state.lock().unwrap().migrate);
        assert!(disk(host).is_empty());
    });
}
