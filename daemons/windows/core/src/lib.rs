//! What the task bridge (`docs/windows.md`) reads from ActivityTaskManager:
//! the `RunningTaskInfo` its task callbacks carry, as the framework's Java
//! code writes it into a Parcel.
//!
//! The layouts are those of the pinned image's framework
//! (`android-16.0.0_r1`): `TaskInfo.writeTaskToParcel`,
//! `Intent.writeToParcel` (with `android.security.prevent_intent_redirect`,
//! which the image enables), `ComponentName`, `Uri`, `BaseBundle` and
//! `ActivityManager.TaskDescription`. Only the leading fields are read: the
//! task, its display, its activities' packages and its label. A base
//! intent carrying `ClipData` is not read further (its layout is long and
//! a launched task's intent does not have one).

/// Reading a Parcel the way Java's `Parcel` reads what it wrote.
pub trait Read {
    fn int(&mut self) -> Result<i32, Error>;
    fn long(&mut self) -> Result<i64, Error>;
    /// `readString` (UTF-16).
    fn string16(&mut self) -> Result<Option<String>, Error>;
    /// Step over a strong binder.
    fn binder(&mut self) -> Result<(), Error>;
    /// Step over `n` bytes of plain data.
    fn skip(&mut self, n: usize) -> Result<(), Error>;

    fn boolean(&mut self) -> Result<bool, Error> {
        Ok(self.int()? != 0)
    }

    /// `readString8`: a byte length, the bytes and a NUL, padded to 4.
    fn string8(&mut self) -> Result<Option<String>, Error> {
        let len = self.int()?;
        if len < 0 {
            return Ok(None);
        }
        let len = len as usize;
        let mut bytes = Vec::with_capacity(len + 4);
        while bytes.len() < len + 1 {
            bytes.extend_from_slice(&self.int()?.to_le_bytes());
        }
        bytes.truncate(len);
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| Error::Malformed)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The parcel ended or does not hold what the layout says.
    Malformed,
    /// A field this reader does not read (`ClipData` in an intent).
    Unsupported,
}

/// The leading fields of a `RunningTaskInfo`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskInfo {
    pub task_id: i32,
    pub display_id: i32,
    /// Whether an activity of the task runs (`isRunning`): a task restored
    /// from recents has none until it is started again.
    pub running: bool,
    /// The package of the task's root activity (`baseActivity`, else
    /// `realActivity`).
    pub package: Option<String>,
    /// The activity the task was started with, `package/class`: the
    /// component its intent named (`origActivity`, an alias), else
    /// `realActivity`.
    pub activity: Option<String>,
    /// `taskDescription.getLabel()`.
    pub label: Option<String>,
}

/// What an activity's requested orientation asks of its window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Orientation {
    /// Nothing fixed: the window keeps the user's proportions.
    #[default]
    Any,
    Landscape,
    Portrait,
}

impl Orientation {
    /// An `ActivityInfo.SCREEN_ORIENTATION_*` value, as
    /// `ActivityInfo.isFixedOrientationLandscape` and `...Portrait` group
    /// them.
    pub fn from_screen_orientation(o: i32) -> Orientation {
        match o {
            // LANDSCAPE, SENSOR_LANDSCAPE, REVERSE_LANDSCAPE, USER_LANDSCAPE
            0 | 6 | 8 | 11 => Orientation::Landscape,
            // PORTRAIT, SENSOR_PORTRAIT, REVERSE_PORTRAIT, USER_PORTRAIT
            1 | 7 | 9 | 12 => Orientation::Portrait,
            _ => Orientation::Any,
        }
    }
}

/// A `ComponentName` written by `ComponentName.writeToParcel(c, out)`:
/// its package and class, or None for null.
pub fn component(r: &mut impl Read) -> Result<Option<(String, String)>, Error> {
    let Some(package) = r.string16()? else {
        return Ok(None);
    };
    let class = r.string16()?.unwrap_or_default();
    Ok(Some((package, class)))
}

/// A `Bundle` written by `writeBundle`: a length (-1 null, 0 empty), then
/// a magic number and that many bytes.
fn bundle(r: &mut impl Read) -> Result<(), Error> {
    let len = r.int()?;
    if len > 0 {
        r.int()?;
        r.skip(len as usize)?;
    }
    Ok(())
}

/// An `Intent` after its typed-object marker.
fn intent(r: &mut impl Read) -> Result<(), Error> {
    r.string8()?; // action
    if r.int()? != 0 {
        r.string8()?; // Uri, of any kind: its string
    }
    r.string8()?; // type
    r.string8()?; // identifier
    r.int()?; // flags
    r.int()?; // extended flags
    r.string8()?; // package
    component(r)?;
    if r.int()? != 0 {
        r.skip(16)?; // source bounds
    }
    for _ in 0..r.int()?.max(0) {
        r.string8()?; // categories
    }
    if r.int()? != 0 {
        intent(r)?; // selector
    }
    if r.int()? != 0 {
        return Err(Error::Unsupported); // ClipData
    }
    r.int()?; // content user hint
    bundle(r)?; // extras
    if r.int()? != 0 {
        intent(r)?; // original intent
    }
    if r.int()? != 0 {
        // Creator token and its nested intent keys.
        r.binder()?;
        for _ in 0..r.int()?.max(0) {
            r.int()?;
            r.string8()?;
            r.int()?;
        }
    }
    Ok(())
}

/// A `RunningTaskInfo` written by `writeTypedObject`. None for a null one.
pub fn running_task_info(r: &mut impl Read) -> Result<Option<TaskInfo>, Error> {
    if r.int()? == 0 {
        return Ok(None);
    }
    r.int()?; // RunningTaskInfo.id, the task id again
    r.int()?; // userId
    let task_id = r.int()?;
    r.int()?; // effectiveUid
    let display_id = r.int()?;
    let running = r.boolean()?;
    if r.int()? != 0 {
        intent(r)?; // baseIntent
    }
    let base = component(r)?;
    component(r)?; // topActivity
    let orig = component(r)?;
    let real = component(r)?;
    r.int()?; // numActivities
    r.long()?; // lastActiveTime
    let mut label = None;
    if r.int()? != 0 && r.int()? != 0 {
        label = r.string16()?;
    }
    Ok(Some(TaskInfo {
        task_id,
        display_id,
        running,
        package: base.or(real.clone()).map(|(p, _)| p),
        activity: orig.or(real).map(|(p, c)| format!("{p}/{c}")),
        label,
    }))
}

/// Encodes what Java's `Parcel` writes, for tests and for the few values
/// the bridge writes itself.
pub mod write {
    /// `writeString8`, as 32-bit words: a byte length, the bytes and a
    /// NUL, zero-padded to 4 (-1 for null).
    pub fn string8(s: Option<&str>) -> Vec<i32> {
        let Some(s) = s else { return vec![-1] };
        let mut bytes = s.as_bytes().to_vec();
        bytes.push(0);
        while bytes.len() % 4 != 0 {
            bytes.push(0);
        }
        let mut words = vec![s.len() as i32];
        words.extend(
            bytes
                .chunks_exact(4)
                .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]])),
        );
        words
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A parcel's bytes, read as libbinder's `Parcel` reads them.
    struct Bytes<'a>(&'a [u8]);

    impl Read for Bytes<'_> {
        fn int(&mut self) -> Result<i32, Error> {
            let (v, rest) = self.0.split_first_chunk::<4>().ok_or(Error::Malformed)?;
            self.0 = rest;
            Ok(i32::from_le_bytes(*v))
        }
        fn long(&mut self) -> Result<i64, Error> {
            let (v, rest) = self.0.split_first_chunk::<8>().ok_or(Error::Malformed)?;
            self.0 = rest;
            Ok(i64::from_le_bytes(*v))
        }
        fn string16(&mut self) -> Result<Option<String>, Error> {
            let len = self.int()?;
            if len < 0 {
                return Ok(None);
            }
            let n = (len as usize + 1) * 2;
            let padded = n.div_ceil(4) * 4;
            let b = self.0.get(..padded).ok_or(Error::Malformed)?;
            let units: Vec<u16> = b[..n - 2]
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            self.0 = &self.0[padded..];
            String::from_utf16(&units)
                .map(Some)
                .map_err(|_| Error::Malformed)
        }
        fn binder(&mut self) -> Result<(), Error> {
            Err(Error::Malformed)
        }
        fn skip(&mut self, n: usize) -> Result<(), Error> {
            self.0 = self.0.get(n..).ok_or(Error::Malformed)?;
            Ok(())
        }
    }

    /// Java's `Parcel` writes, into bytes.
    #[derive(Default)]
    struct Out(Vec<u8>);

    impl Out {
        fn int(&mut self, v: i32) -> &mut Self {
            self.0.extend_from_slice(&v.to_le_bytes());
            self
        }
        fn long(&mut self, v: i64) -> &mut Self {
            self.0.extend_from_slice(&v.to_le_bytes());
            self
        }
        fn string8(&mut self, s: Option<&str>) -> &mut Self {
            for w in write::string8(s) {
                self.int(w);
            }
            self
        }
        fn string16(&mut self, s: Option<&str>) -> &mut Self {
            let Some(s) = s else { return self.int(-1) };
            let units: Vec<u16> = s.encode_utf16().collect();
            self.int(units.len() as i32);
            for u in units.iter().chain([&0]) {
                self.0.extend_from_slice(&u.to_le_bytes());
            }
            while self.0.len() % 4 != 0 {
                self.0.push(0);
            }
            self
        }
        fn component(&mut self, c: Option<(&str, &str)>) -> &mut Self {
            match c {
                Some((p, c)) => self.string16(Some(p)).string16(Some(c)),
                None => self.string16(None),
            }
        }
        /// A launcher's intent for `package`, with an extras bundle of
        /// `extras` bytes when given.
        fn intent(&mut self, package: &str, extras: Option<usize>) -> &mut Self {
            self.string8(Some("android.intent.action.MAIN"))
                .int(1)
                .string8(Some("content://x/y"))
                .string8(None)
                .string8(None)
                .int(0x10200000)
                .int(0)
                .string8(None)
                .component(Some((package, ".Main")))
                .int(1)
                .int(0)
                .int(0)
                .int(10)
                .int(20)
                .int(1)
                .string8(Some("android.intent.category.LAUNCHER"))
                .int(0)
                .int(0)
                .int(-2);
            match extras {
                Some(n) => {
                    self.int(n as i32).int(0x4C444E42);
                    self.0.extend(std::iter::repeat_n(7, n));
                }
                None => {
                    self.int(-1);
                }
            }
            self.int(0).int(0)
        }
    }

    fn task(label: Option<&str>, extras: Option<usize>) -> Vec<u8> {
        let mut o = Out::default();
        o.int(1).int(42).int(0).int(42).int(10123).int(0).int(1);
        o.int(1).intent("org.example", extras);
        o.component(Some(("org.example", "org.example.Main")))
            .component(Some(("org.example", "org.example.Other")))
            .component(None)
            .component(Some(("org.example", "org.example.Main")))
            .int(2)
            .long(123_456_789);
        o.int(1);
        match label {
            Some(l) => o.int(1).string16(Some(l)),
            None => o.int(0),
        };
        // The rest of the task description and the task info.
        o.int(0).int(-1).int(0);
        o.0
    }

    #[test]
    fn reads_the_task_its_package_and_label() {
        let bytes = task(Some("Settings — Wi‑Fi"), None);
        let info = running_task_info(&mut Bytes(&bytes)).unwrap().unwrap();
        assert_eq!(
            info,
            TaskInfo {
                task_id: 42,
                display_id: 0,
                running: true,
                package: Some("org.example".into()),
                activity: Some("org.example/org.example.Main".into()),
                label: Some("Settings — Wi‑Fi".into()),
            }
        );
        let bytes = task(None, Some(12));
        let info = running_task_info(&mut Bytes(&bytes)).unwrap().unwrap();
        assert_eq!(info.label, None);
        assert_eq!(info.package.as_deref(), Some("org.example"));
    }

    #[test]
    fn an_alias_is_the_activity_started() {
        let mut o = Out::default();
        o.int(1).int(7).int(0).int(7);
        o.int(10123).int(0).int(1).int(0);
        o.component(Some(("org.example", "org.example.Real")))
            .component(None)
            .component(Some(("org.example", "org.example.Alias")))
            .component(Some(("org.example", "org.example.Real")))
            .int(1)
            .long(1)
            .int(0);
        let info = running_task_info(&mut Bytes(&o.0)).unwrap().unwrap();
        assert_eq!(
            info.activity.as_deref(),
            Some("org.example/org.example.Alias")
        );
    }

    #[test]
    fn null_and_clip_data() {
        assert_eq!(running_task_info(&mut Bytes(&0i32.to_le_bytes())), Ok(None));
        let mut o = Out::default();
        o.int(1).int(1).int(0).int(1).int(0).int(0).int(1).int(1);
        o.string8(None)
            .int(0)
            .string8(None)
            .string8(None)
            .int(0)
            .int(0);
        o.string8(None).component(None).int(0).int(0).int(0).int(1);
        assert_eq!(running_task_info(&mut Bytes(&o.0)), Err(Error::Unsupported));
        assert_eq!(
            running_task_info(&mut Bytes(&o.0[..20])),
            Err(Error::Malformed)
        );
    }

    #[test]
    fn fixed_orientations() {
        use Orientation::*;
        for (o, want) in [
            (-1, Any), // UNSPECIFIED
            (0, Landscape),
            (1, Portrait),
            (2, Any), // USER
            (4, Any), // SENSOR
            (6, Landscape),
            (7, Portrait),
            (8, Landscape),
            (9, Portrait),
            (10, Any), // FULL_SENSOR
            (11, Landscape),
            (12, Portrait),
            (14, Any), // LOCKED
        ] {
            assert_eq!(Orientation::from_screen_orientation(o), want, "{o}");
        }
    }

    #[test]
    fn string8_words() {
        assert_eq!(write::string8(None), [-1]);
        assert_eq!(write::string8(Some("")), [0, 0]);
        assert_eq!(
            write::string8(Some("abc")),
            [3, i32::from_le_bytes(*b"abc\0")]
        );
        assert_eq!(write::string8(Some("abcd")).len(), 3);
        let mut o = Out::default();
        o.string8(Some("héllo")).int(7);
        let mut r = Bytes(&o.0);
        assert_eq!(r.string8().unwrap().as_deref(), Some("héllo"));
        assert_eq!(r.int(), Ok(7));
    }
}
