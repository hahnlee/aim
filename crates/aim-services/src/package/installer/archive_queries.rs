//! Original image LauncherApps/AppOps/graphics leaves for native archive queries.
use aim_binder_host::{
    local::Strong,
    parcel::{BAD_VALUE, EX_ILLEGAL_STATE, Exception, Parcel, Reader},
};
use aim_service_aidl::{
    ReadParcelable, WriteParcelable, dev_aim_server_iinstallerarchivequerybridge as api,
};
use std::{path::Path, sync::Arc};
fn error(message: impl Into<String>) -> Exception {
    Exception::new(EX_ILLEGAL_STATE, message)
}
pub struct Leaf {
    owner: Strong,
    current: Arc<dyn Fn() -> Result<(), Exception> + Send + Sync>,
}
impl Leaf {
    pub fn new(
        owner: Strong,
        current: Arc<dyn Fn() -> Result<(), Exception> + Send + Sync>,
    ) -> Arc<Self> {
        Arc::new(Self { owner, current })
    }
    fn call<T>(
        &self,
        code: u32,
        parcel: &Parcel,
        read: impl FnOnce(&mut Reader<'_>) -> Result<T, i32>,
    ) -> Result<T, Exception> {
        (self.current)()?;
        let reply = self
            .owner
            .transact(code, parcel, false)
            .map_err(|code| error(format!("archive query transport {code}")))?;
        let mut reader = reply.reader();
        reader
            .read_exception()
            .map_err(|code| error(format!("archive query exception {code}")))??;
        let value =
            read(&mut reader).map_err(|code| error(format!("archive query payload {code}")))?;
        if reader.remaining() != 0 {
            return Err(error("trailing archive query reply"));
        }
        (self.current)()?;
        Ok(value)
    }
    fn activities(
        &self,
        name: &str,
        user: i32,
    ) -> Result<Vec<crate::package::archive::Activity>, Exception> {
        let mut request = Parcel::new();
        api::GetLauncherActivities {
            package_name: Some(name.into()),
            user_id: user,
        }
        .write(&mut request);
        let bytes = self.call(api::GET_LAUNCHER_ACTIVITIES, &request, |reader| {
            aim_service_aidl::read_byte_array(reader)?.ok_or(BAD_VALUE)
        })?;
        let mut reader = Reader::new(&bytes, &[]);
        if reader
            .read_i32()
            .map_err(|_| error("archive activities version"))?
            != 1
        {
            return Err(error("archive activities version"));
        }
        let count = reader
            .read_i32()
            .map_err(|_| error("archive activities count"))?;
        if count < 0 || count as usize > reader.remaining() / 16 {
            return Err(error("archive activities count"));
        }
        let mut result = Vec::new();
        for _ in 0..count {
            result.push(crate::package::archive::Activity {
                title: reader
                    .read_string16()
                    .map_err(|_| error("archive activity title"))?
                    .ok_or_else(|| error("null archive activity title"))?,
                component: crate::package::intent::ComponentName {
                    package: reader
                        .read_string16()
                        .map_err(|_| error("archive component package"))?
                        .ok_or_else(|| error("null archive package"))?,
                    class: reader
                        .read_string16()
                        .map_err(|_| error("archive component class"))?
                        .ok_or_else(|| error("null archive class"))?,
                },
                icon: aim_service_aidl::read_byte_array(&mut reader)
                    .map_err(|_| error("archive icon"))?,
                monochrome: aim_service_aidl::read_byte_array(&mut reader)
                    .map_err(|_| error("archive monochrome"))?,
            });
        }
        if reader.remaining() != 0 {
            return Err(error("trailing launcher archive activities"));
        }
        Ok(result)
    }
    fn op(&self, overlay: bool, uid: i32, name: &str) -> Result<i32, Exception> {
        let mut request = Parcel::new();
        let code = if overlay {
            api::GetArchiveOverlayMode {
                uid,
                package_name: Some(name.into()),
            }
            .write(&mut request);
            api::GET_ARCHIVE_OVERLAY_MODE
        } else {
            api::GetArchiveOptOutMode {
                uid,
                package_name: Some(name.into()),
            }
            .write(&mut request);
            api::GET_ARCHIVE_OPT_OUT_MODE
        };
        self.call(code, &request, |reader| reader.read_i32())
    }
    fn overlay(
        &self,
        icon: crate::package::instant::Bitmap,
        density: i32,
    ) -> Result<crate::package::instant::Bitmap, Exception> {
        let mut request = Parcel::new();
        api::IncludeArchiveOverlay { icon: Some(icon) }.write(&mut request);
        let png = self.call(api::INCLUDE_ARCHIVE_OVERLAY, &request, |reader| {
            aim_service_aidl::read_byte_array(reader)?.ok_or(BAD_VALUE)
        })?;
        crate::package::instant::Bitmap::decode(&png, density)
            .map_err(error)?
            .ok_or_else(|| error("original archive overlay is not decodable"))
    }
    pub fn owner(
        self: &Arc<Self>,
        data: &Path,
        density: i32,
    ) -> Arc<crate::package::archive::Owner> {
        let launcher = self.clone();
        let overlay_op = self.clone();
        let opt_out = self.clone();
        let graphics = self.clone();
        Arc::new(crate::package::archive::Owner::new(
            data,
            density,
            Box::new(move |name, user| launcher.activities(name, user)),
            Box::new(move |uid, name| overlay_op.op(true, uid, name)),
            Box::new(move |uid, name| opt_out.op(false, uid, name)),
            Box::new(move |icon| graphics.overlay(icon, density)),
        ))
    }
    pub fn draft_params(
        &self,
        name: &str,
        user: i32,
        launcher_uid: i32,
        launcher_package: &str,
    ) -> Result<super::codec::SessionParams, Exception> {
        let mut request = Parcel::new();
        api::GetUnarchiveDraftParams {
            package_name: Some(name.into()),
            user_id: user,
            launcher_uid,
            launcher_package: Some(launcher_package.into()),
        }
        .write(&mut request);
        let bytes = self.call(api::GET_UNARCHIVE_DRAFT_PARAMS, &request, |reader| {
            aim_service_aidl::read_byte_array(reader)?.ok_or(BAD_VALUE)
        })?;
        let mut reader = Reader::new(&bytes, &[]);
        let params = super::codec::SessionParams::read_from(&mut reader)
            .map_err(|_| error("unarchive draft SessionParams"))?;
        if reader.remaining() != 0 {
            return Err(error("trailing unarchive draft params"));
        }
        Ok(params)
    }
}
