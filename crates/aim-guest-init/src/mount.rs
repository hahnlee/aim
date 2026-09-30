//! `/data`, `/metadata` and `/cache`: the data directory's image
//! (docs/storage.md), attached on another thread from the boot's start and
//! mounted for the guest when init's first `mount_all` runs, in the `fs`
//! stage, as a device mounts its partitions there. What runs before only
//! needs the image and the runtime directory.

use std::path::PathBuf;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use aim_storage::data::DataImage;

use crate::paths::{Layout, Sweep};

pub struct DataMount {
    layout: Layout,
    /// The boot's start.
    epoch: Instant,
    /// Run mode: the attach, until it is waited for, and when it ended.
    pending: Option<JoinHandle<Result<(DataImage, Instant), String>>>,
    /// When the attach ended, and when the mount did and how long it
    /// waited for the attach, since the boot's start.
    attached: Option<Duration>,
    mounted: Option<(Duration, Duration)>,
    /// Declared before the image: finishes before it is detached.
    sweep: Option<Sweep>,
    image: Option<DataImage>,
}

impl DataMount {
    /// Starts attaching the image of `dir` (run mode), or lays the data
    /// directory out in place (`None`, a dry run).
    pub fn start(layout: Layout, dir: Option<PathBuf>, epoch: Instant) -> Result<Self, String> {
        let pending = dir
            .map(|dir| {
                std::thread::Builder::new()
                    .name("data-attach".into())
                    .spawn(move || DataImage::attach(&dir).map(|image| (image, Instant::now())))
                    .map_err(|e| format!("data image: {e}"))
            })
            .transpose()?;
        Ok(Self {
            layout,
            epoch,
            pending,
            attached: None,
            mounted: None,
            sweep: None,
            image: None,
        })
    }

    /// Waits for the attach and lays out the persistent directories, once;
    /// returns how long it waited.
    pub fn mount(&mut self) -> Result<Duration, String> {
        let start = Instant::now();
        if self.mounted.is_some() {
            return Ok(Duration::ZERO);
        }
        if let Some(pending) = self.pending.take() {
            let (image, attached) = pending
                .join()
                .map_err(|_| "data image: the attach panicked".to_string())??;
            self.attached = Some(attached - self.epoch);
            if image.dir() != self.layout.data {
                return Err(format!(
                    "data image mounted at {}, not {}",
                    image.dir().display(),
                    self.layout.data.display()
                ));
            }
            self.image = Some(image);
        }
        self.sweep = Some(self.layout.prepare_data().map_err(|e| e.to_string())?);
        let waited = start.elapsed();
        self.mounted = Some((self.epoch.elapsed(), waited));
        Ok(waited)
    }

    /// The attach's and the mount's timeline entries.
    pub fn timeline(&self) -> Vec<(Duration, String)> {
        let mut out = Vec::new();
        if let Some(at) = self.attached {
            out.push((at, "data image attached".to_string()));
        }
        if let Some((at, waited)) = self.mounted {
            out.push((
                at,
                format!("data mounted, after waiting {:.3} s", waited.as_secs_f64()),
            ));
        }
        out
    }
}

impl Drop for DataMount {
    /// An attach still running finishes, and its image is detached.
    fn drop(&mut self) {
        if let Some(pending) = self.pending.take() {
            let _ = pending.join();
        }
    }
}
