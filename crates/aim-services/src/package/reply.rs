//! The replies of the package queries decoded for the shadow comparison
//! (`ShadowModel::decode_reply`): `PackageInfo`, `ApplicationInfo` and
//! the component infos as their parcel constructors read them at
//! `android-16.0.0_r1`, field by field, so a difference names its field.
//! A squashed `ApplicationInfo` (`Parcel.readSquashed`) reads as the one
//! it points back to, and `createTimestamp`, which no two replies share,
//! is left out.

use std::collections::HashMap;

use aim_binder_host::parcel::{BAD_VALUE, Reader, Result};
use aim_service_aidl::android_content_pm_ipackagemanager as pm;

use crate::shadow::Value;

/// `Parcel.VAL_*` read in bundles and lists.
const VAL_NULL: i32 = -1;
const VAL_STRING: i32 = 0;
const VAL_INTEGER: i32 = 1;
const VAL_PARCELABLE: i32 = 4;
const VAL_LONG: i32 = 6;
const VAL_FLOAT: i32 = 7;
const VAL_DOUBLE: i32 = 8;
const VAL_BOOLEAN: i32 = 9;

/// The reply of `code` of `descriptor`, decoded; `None` for a method
/// compared raw.
pub fn decode(descriptor: &str, code: u32, r: &mut Reader<'_>) -> Option<Result<Value>> {
    if descriptor != pm::DESCRIPTOR {
        return None;
    }
    let item: fn(&mut D<'_, '_>) -> Result<()> = match code {
        pm::GET_PACKAGE_INFO => |d| d.package_info(),
        pm::GET_APPLICATION_INFO => |d| d.application_info("info"),
        pm::GET_ACTIVITY_INFO | pm::GET_RECEIVER_INFO => |d| d.activity_info(),
        pm::GET_SERVICE_INFO => |d| d.service_info(),
        pm::GET_PROVIDER_INFO => |d| d.provider_info(),
        pm::GET_INSTALLED_PACKAGES => |d| d.list(|d| d.package_info()),
        pm::GET_INSTALLED_APPLICATIONS => |d| d.list(|d| d.application_info("info")),
        _ => return None,
    };
    Some(read_reply(r, item))
}

fn read_reply(r: &mut Reader<'_>, item: fn(&mut D<'_, '_>) -> Result<()>) -> Result<Value> {
    if let Err(e) = r.read_exception()? {
        return Ok(Value::Exception {
            code: e.code,
            message: e.message,
        });
    }
    if r.read_i32()? == 0 {
        return Ok(Value::Null);
    }
    let mut squashed = HashMap::new();
    let mut d = D::new(r, &mut squashed);
    item(&mut d)?;
    Ok(d.done())
}

/// An `ActivityInfo` (or a receiver's) as a value; m4resolve's
/// `ResolveInfo` reads its component infos with these.
pub fn activity_info(r: &mut Reader<'_>) -> Result<Value> {
    one(r, |d| d.activity_info())
}

pub fn service_info(r: &mut Reader<'_>) -> Result<Value> {
    one(r, |d| d.service_info())
}

pub fn provider_info(r: &mut Reader<'_>) -> Result<Value> {
    one(r, |d| d.provider_info())
}

fn one(r: &mut Reader<'_>, item: fn(&mut D<'_, '_>) -> Result<()>) -> Result<Value> {
    let mut squashed = HashMap::new();
    let mut d = D::new(r, &mut squashed);
    item(&mut d)?;
    Ok(d.done())
}

/// A parcelable's fields as they are read, and the squashed
/// `ApplicationInfo`s read so far, by position.
struct D<'r, 'a> {
    r: &'r mut Reader<'a>,
    squashed: &'r mut HashMap<usize, Value>,
    fields: Vec<(String, Value)>,
}

impl<'r, 'a> D<'r, 'a> {
    fn new(r: &'r mut Reader<'a>, squashed: &'r mut HashMap<usize, Value>) -> Self {
        D {
            r,
            squashed,
            fields: Vec::new(),
        }
    }

    fn done(self) -> Value {
        if let [(name, v)] = &self.fields[..]
            && name == "info"
        {
            return v.clone();
        }
        Value::Fields(self.fields)
    }

    fn push(&mut self, name: &str, v: Value) {
        self.fields.push((name.into(), v));
    }

    /// A nested parcelable, read by `read` into its own fields.
    fn nested(
        &mut self,
        name: &str,
        read: impl FnOnce(&mut D<'_, 'a>) -> Result<()>,
    ) -> Result<()> {
        let mut d = D::new(&mut *self.r, &mut *self.squashed);
        read(&mut d)?;
        let v = Value::Fields(d.fields);
        self.push(name, v);
        Ok(())
    }

    fn int(&mut self, name: &str) -> Result<i32> {
        let v = self.r.read_i32()?;
        self.push(name, Value::Int(v));
        Ok(v)
    }

    fn ints(&mut self, names: &[&str]) -> Result<()> {
        names.iter().try_for_each(|n| self.int(n).map(drop))
    }

    fn long(&mut self, name: &str) -> Result<()> {
        let v = self.r.read_i64()?;
        self.push(name, Value::Long(v));
        Ok(())
    }

    fn float(&mut self, name: &str) -> Result<()> {
        let v = self.r.read_f32()?;
        self.push(name, Value::Float(v.to_bits()));
        Ok(())
    }

    fn bool(&mut self, name: &str) -> Result<()> {
        let v = self.r.read_i32()? != 0;
        self.push(name, Value::Bool(v));
        Ok(())
    }

    fn str_value(s: Option<String>) -> Value {
        s.map_or(Value::Null, Value::Str)
    }

    fn s8(&mut self, name: &str) -> Result<()> {
        let s = self.r.read_string8()?;
        self.push(name, Self::str_value(s));
        Ok(())
    }

    fn s8s(&mut self, names: &[&str]) -> Result<()> {
        names.iter().try_for_each(|n| self.s8(n))
    }

    fn s16(&mut self, name: &str) -> Result<()> {
        let s = self.r.read_string16()?;
        self.push(name, Self::str_value(s));
        Ok(())
    }

    /// A count (-1 null), then the items as `item` reads them.
    fn array(
        &mut self,
        name: &str,
        mut item: impl FnMut(&mut Reader<'_>) -> Result<Value>,
    ) -> Result<()> {
        let n = self.r.read_i32()?;
        let v = if n < 0 {
            Value::Null
        } else {
            Value::List((0..n).map(|_| item(self.r)).collect::<Result<_>>()?)
        };
        self.push(name, v);
        Ok(())
    }

    fn s8_array(&mut self, name: &str) -> Result<()> {
        self.array(name, |r| Ok(Self::str_value(r.read_string8()?)))
    }

    fn s16_array(&mut self, name: &str) -> Result<()> {
        self.array(name, |r| Ok(Self::str_value(r.read_string16()?)))
    }

    fn int_array(&mut self, name: &str) -> Result<()> {
        self.array(name, |r| Ok(Value::Int(r.read_i32()?)))
    }

    fn bytes(r: &mut Reader<'_>) -> Result<Value> {
        let n = r.read_i32()?;
        if n < 0 {
            return Ok(Value::Null);
        }
        let at = r.position();
        r.skip(n as usize)?;
        Ok(Value::Bytes(r.since(at).0[..n as usize].to_vec()))
    }

    /// `readTypedArray`/`createTypedArrayList`: each item behind whether it
    /// is there.
    fn typed_array(
        &mut self,
        name: &str,
        mut item: impl FnMut(&mut D<'_, 'a>) -> Result<()>,
    ) -> Result<()> {
        let n = self.r.read_i32()?;
        if n < 0 {
            self.push(name, Value::Null);
            return Ok(());
        }
        let mut items = Vec::new();
        for _ in 0..n {
            if self.r.read_i32()? == 0 {
                items.push(Value::Null);
                continue;
            }
            let mut d = D::new(&mut *self.r, &mut *self.squashed);
            item(&mut d)?;
            items.push(d.done());
        }
        self.push(name, Value::List(items));
        Ok(())
    }

    /// `TextUtils.CHAR_SEQUENCE_CREATOR` of a plain string.
    fn char_sequence(&mut self, name: &str) -> Result<()> {
        let kind = self.r.read_i32()?;
        let s = self.r.read_string8()?;
        if s.is_some() && kind != 1 {
            return Err(BAD_VALUE);
        }
        self.push(name, Self::str_value(s));
        Ok(())
    }

    /// A value of `Parcel.readValue` in a bundle.
    fn value(r: &mut Reader<'_>) -> Result<Value> {
        Ok(match r.read_i32()? {
            VAL_NULL => Value::Null,
            VAL_STRING => Self::str_value(r.read_string16()?),
            VAL_INTEGER => Value::Int(r.read_i32()?),
            VAL_LONG => Value::Long(r.read_i64()?),
            VAL_FLOAT => Value::Float(r.read_f32()?.to_bits()),
            VAL_DOUBLE => Value::Long(r.read_i64()?),
            VAL_BOOLEAN => Value::Bool(r.read_i32()? != 0),
            _ => return Err(BAD_VALUE),
        })
    }

    /// `readBundle`: its entries, or its bytes when a value is not one a
    /// manifest gives.
    fn bundle(&mut self, name: &str) -> Result<()> {
        let len = self.r.read_i32()?;
        if len <= 0 {
            self.push(
                name,
                if len < 0 {
                    Value::Null
                } else {
                    Value::List(vec![])
                },
            );
            return Ok(());
        }
        let start = self.r.position();
        let end = start + 4 + len as usize;
        let entries = (|| {
            self.r.read_i32()?;
            let n = self.r.read_i32()?;
            (0..n)
                .map(|_| {
                    let key = self.r.read_string16()?.unwrap_or_default();
                    Ok((key, Self::value(self.r)?))
                })
                .collect::<Result<Vec<_>>>()
        })();
        let v = match entries {
            Ok(entries) if self.r.position() == end => Value::Fields(entries),
            _ => {
                self.r.set_position(start);
                self.r.skip(4 + len as usize)?;
                Value::Bytes(self.r.since(start).0.to_vec())
            }
        };
        self.r.read_i32()?;
        self.push(name, v);
        Ok(())
    }

    /// `PackageItemInfo(Parcel)`.
    fn item(&mut self) -> Result<()> {
        self.s8s(&["name", "packageName"])?;
        self.int("labelRes")?;
        self.char_sequence("nonLocalizedLabel")?;
        self.ints(&["icon", "logo"])?;
        self.bundle("metaData")?;
        self.ints(&["banner", "showUserIcon"])?;
        self.bool("isArchived")
    }

    /// `ApplicationInfo.CREATOR` (`readSquashed`).
    fn application_info(&mut self, name: &str) -> Result<()> {
        let offset = self.r.read_i32()? as usize;
        let at = self.r.position();
        if offset != 0 {
            let v = self
                .squashed
                .get(&(at - offset))
                .cloned()
                .ok_or(BAD_VALUE)?;
            self.push(name, v);
            return Ok(());
        }
        let mut d = D::new(&mut *self.r, &mut *self.squashed);
        d.application_info_fields()?;
        let v = Value::Fields(d.fields);
        self.squashed.insert(at, v.clone());
        self.push(name, v);
        Ok(())
    }

    fn application_info_fields(&mut self) -> Result<()> {
        self.item()?;
        self.s8s(&["taskAffinity", "permission", "processName", "className"])?;
        self.ints(&[
            "theme",
            "flags",
            "privateFlags",
            "privateFlagsExt",
            "requiresSmallestWidthDp",
            "compatibleWidthLimitDp",
            "largestWidthLimitDp",
        ])?;
        if self.r.read_i32()? != 0 {
            self.long("storageUuidMsb")?;
            self.long("storageUuidLsb")?;
        }
        self.s8s(&[
            "scanSourceDir",
            "scanPublicSourceDir",
            "sourceDir",
            "publicSourceDir",
        ])?;
        self.s8_array("splitNames")?;
        self.s8_array("splitSourceDirs")?;
        self.s8_array("splitPublicSourceDirs")?;
        self.sparse_int_arrays("splitDependencies")?;
        self.s8s(&[
            "nativeLibraryDir",
            "secondaryNativeLibraryDir",
            "nativeLibraryRootDir",
        ])?;
        self.bool("nativeLibraryRootRequiresIsa")?;
        self.s8s(&["primaryCpuAbi", "secondaryCpuAbi"])?;
        self.s8_array("resourceDirs")?;
        self.s8_array("overlayPaths")?;
        self.s8s(&["seInfo", "seInfoUser"])?;
        self.s8_array("sharedLibraryFiles")?;
        self.typed_array("sharedLibraryInfos", |d| d.shared_library())?;
        self.typed_array("optionalSharedLibraryInfos", |d| d.shared_library())?;
        self.s8s(&[
            "dataDir",
            "deviceProtectedDataDir",
            "credentialProtectedDataDir",
        ])?;
        self.ints(&["uid", "minSdkVersion", "targetSdkVersion"])?;
        self.long("longVersionCode")?;
        self.bool("enabled")?;
        self.ints(&["enabledSetting", "installLocation"])?;
        self.s8s(&["manageSpaceActivityName", "backupAgentName"])?;
        self.ints(&[
            "descriptionRes",
            "uiOptions",
            "fullBackupContent",
            "dataExtractionRulesRes",
        ])?;
        self.bool("crossProfile")?;
        self.ints(&[
            "networkSecurityConfigRes",
            "category",
            "targetSandboxVersion",
        ])?;
        self.s8("classLoaderName")?;
        self.s8_array("splitClassLoaderNames")?;
        self.int("compileSdkVersion")?;
        self.s8s(&["compileSdkVersionCodename", "appComponentFactory"])?;
        self.ints(&["iconRes", "roundIconRes", "hiddenApiPolicy"])?;
        self.bool("hiddenUntilInstalled")?;
        self.s8("zygotePreloadName")?;
        self.ints(&[
            "gwpAsanMode",
            "memtagMode",
            "nativeHeapZeroInitialized",
            "requestRawExternalStorageAccess",
        ])?;
        // createTimestamp.
        self.r.read_i64()?;
        let n = self.r.read_i32()?;
        let map = (0..n)
            .map(|_| {
                Ok(Value::List(vec![
                    Self::str_value(self.r.read_string16()?),
                    Self::str_value(self.r.read_string16()?),
                ]))
            })
            .collect::<Result<_>>()?;
        self.push("appClassNamesByProcess", Value::List(map));
        self.int("localeConfigRes")?;
        self.bool("allowCrossUidActivitySwitchFromBelow")?;
        self.int("pageSizeAppCompatFlags")?;
        self.s16_array("knownActivityEmbeddingCerts")
    }

    /// `readSparseArray` of `int[]`s.
    fn sparse_int_arrays(&mut self, name: &str) -> Result<()> {
        self.array(name, |r| {
            let key = r.read_i32()?;
            let value = match r.read_i32()? {
                VAL_NULL => Value::Null,
                _ => {
                    let n = r.read_i32()?;
                    Value::List(
                        (0..n.max(0))
                            .map(|_| Ok(Value::Int(r.read_i32()?)))
                            .collect::<Result<_>>()?,
                    )
                }
            };
            Ok(Value::List(vec![Value::Int(key), value]))
        })
    }

    /// `readParcelable` of a `VersionedPackage`.
    fn versioned_package(r: &mut Reader<'_>) -> Result<Value> {
        if r.read_string16()?.is_none() {
            return Ok(Value::Null);
        }
        Ok(Value::List(vec![
            Self::str_value(r.read_string8()?),
            Value::Long(r.read_i64()?),
        ]))
    }

    /// `SharedLibraryInfo(Parcel)`.
    fn shared_library(&mut self) -> Result<()> {
        self.s8s(&["path", "packageName"])?;
        if self.r.read_i32()? != 0 {
            self.s8_array("codePaths")?;
        }
        self.s8("name")?;
        self.long("version")?;
        self.int("type")?;
        let declaring = Self::versioned_package(self.r)?;
        self.push("declaringPackage", declaring);
        // readArrayList of VersionedPackages, each a length-prefixed value.
        self.array("dependentPackages", |r| {
            if r.read_i32()? != VAL_PARCELABLE {
                return Err(BAD_VALUE);
            }
            r.read_i32()?;
            Self::versioned_package(r)
        })?;
        self.typed_array("dependencies", |d| d.shared_library())?;
        self.bool("isNative")?;
        self.array("optionalDependentPackages", Self::versioned_package)?;
        self.s16_array("certDigests")
    }

    /// `ComponentInfo(Parcel)`.
    fn component(&mut self) -> Result<()> {
        self.item()?;
        self.application_info("applicationInfo")?;
        self.s8s(&["processName", "splitName"])?;
        self.s8_array("attributionTags")?;
        self.int("descriptionRes")?;
        self.bool("enabled")?;
        self.bool("exported")?;
        self.bool("directBootAware")
    }

    fn activity_info(&mut self) -> Result<()> {
        self.nested("info", |d| {
            d.component()?;
            d.ints(&["theme", "launchMode", "documentLaunchMode"])?;
            d.s8s(&[
                "permission",
                "taskAffinity",
                "targetActivity",
                "launchToken",
            ])?;
            d.ints(&[
                "flags",
                "privateFlags",
                "screenOrientation",
                "configChanges",
                "softInputMode",
                "uiOptions",
            ])?;
            d.s8("parentActivityName")?;
            d.ints(&["persistableMode", "maxRecents", "lockTaskLaunchMode"])?;
            if d.r.read_i32()? != 0 {
                d.int("windowLayout.width")?;
                d.float("windowLayout.widthFraction")?;
                d.int("windowLayout.height")?;
                d.float("windowLayout.heightFraction")?;
                d.ints(&[
                    "windowLayout.gravity",
                    "windowLayout.minWidth",
                    "windowLayout.minHeight",
                ])?;
                d.s8("windowLayout.affinity")?;
            }
            d.int("resizeMode")?;
            d.s8("requestedVrComponent")?;
            d.ints(&["rotationAnimation", "colorMode"])?;
            d.float("maxAspectRatio")?;
            d.float("minAspectRatio")?;
            d.bool("supportsSizeChanges")?;
            d.s16_array("knownActivityEmbeddingCerts")?;
            d.s8("requiredDisplayCategory")?;
            d.int("requireContentUriPermissionFromCaller").map(drop)
        })
    }

    fn service_info(&mut self) -> Result<()> {
        self.nested("info", |d| {
            d.component()?;
            d.s8("permission")?;
            d.ints(&["flags", "foregroundServiceType"])
        })
    }

    fn pattern(d: &mut D<'_, '_>) -> Result<()> {
        d.s16("pattern")?;
        d.int("type")?;
        d.int_array("parsed")
    }

    fn provider_info(&mut self) -> Result<()> {
        self.nested("info", |d| {
            d.component()?;
            d.s8s(&["authority", "readPermission", "writePermission"])?;
            d.bool("grantUriPermissions")?;
            d.bool("forceUriPermissions")?;
            d.typed_array("uriPermissionPatterns", D::pattern)?;
            d.typed_array("pathPermissions", |d| {
                D::pattern(d)?;
                d.s16("readPermission")?;
                d.s16("writePermission")
            })?;
            d.bool("multiprocess")?;
            d.ints(&["initOrder", "flags"])?;
            d.bool("isSyncable")
        })
    }

    fn signing_details(&mut self) -> Result<()> {
        if self.r.read_i32()? != 0 {
            self.push("unknown", Value::Bool(true));
            return Ok(());
        }
        self.array("signatures", |r| {
            if r.read_i32()? == 0 {
                return Ok(Value::Null);
            }
            Self::bytes(r)
        })?;
        self.int("schemeVersion")?;
        // The public keys: serializables, each a length-prefixed value.
        self.array("publicKeys", |r| {
            r.read_i32()?;
            let len = r.read_i32()?;
            let at = r.position();
            r.skip(len.max(0) as usize)?;
            Ok(Value::Bytes(r.since(at).0.to_vec()))
        })?;
        self.array("pastSigningCertificates", |r| {
            if r.read_i32()? == 0 {
                return Ok(Value::Null);
            }
            Self::bytes(r)
        })
    }

    fn feature_info(d: &mut D<'_, '_>) -> Result<()> {
        d.s8("name")?;
        d.ints(&["version", "reqGlEsVersion", "flags"])
    }

    /// `PackageInfo(Parcel)`, whose `ApplicationInfo`s may be squashed.
    fn package_info(&mut self) -> Result<()> {
        self.s8("packageName")?;
        self.s8_array("splitNames")?;
        self.ints(&["versionCode", "versionCodeMajor"])?;
        self.s8("versionName")?;
        self.int("baseRevisionCode")?;
        self.int_array("splitRevisionCodes")?;
        self.s8("sharedUserId")?;
        self.int("sharedUserLabel")?;
        if self.r.read_i32()? != 0 {
            self.application_info("applicationInfo")?;
        }
        self.long("firstInstallTime")?;
        self.long("lastUpdateTime")?;
        self.int_array("gids")?;
        self.typed_array("activities", |d| d.activity_info())?;
        self.typed_array("receivers", |d| d.activity_info())?;
        self.typed_array("services", |d| d.service_info())?;
        self.typed_array("providers", |d| d.provider_info())?;
        self.typed_array("instrumentation", |d| {
            d.item()?;
            d.s8s(&[
                "targetPackage",
                "targetProcesses",
                "sourceDir",
                "publicSourceDir",
            ])?;
            d.s8_array("splitNames")?;
            d.s8_array("splitSourceDirs")?;
            d.s8_array("splitPublicSourceDirs")?;
            d.sparse_int_arrays("splitDependencies")?;
            d.s8s(&[
                "dataDir",
                "deviceProtectedDataDir",
                "credentialProtectedDataDir",
                "primaryCpuAbi",
                "secondaryCpuAbi",
                "nativeLibraryDir",
                "secondaryNativeLibraryDir",
            ])?;
            d.bool("handleProfiling")?;
            d.bool("functionalTest")
        })?;
        self.typed_array("permissions", |d| {
            d.item()?;
            d.ints(&["protectionLevel", "flags"])?;
            d.s8s(&["group", "backgroundPermission"])?;
            d.ints(&["descriptionRes", "requestRes"])?;
            d.char_sequence("nonLocalizedDescription")?;
            d.s16_array("knownCerts")
        })?;
        self.s8_array("requestedPermissions")?;
        self.int_array("requestedPermissionsFlags")?;
        self.array("signatures", |r| {
            if r.read_i32()? == 0 {
                return Ok(Value::Null);
            }
            Self::bytes(r)
        })?;
        self.typed_array("configPreferences", |d| {
            d.ints(&[
                "reqTouchScreen",
                "reqKeyboardType",
                "reqNavigation",
                "reqInputFeatures",
                "reqGlEsVersion",
            ])
        })?;
        self.typed_array("reqFeatures", D::feature_info)?;
        self.typed_array("featureGroups", |d| {
            d.typed_array("features", D::feature_info)
        })?;
        self.typed_array("attributions", |d| {
            d.s16("tag")?;
            d.int("label").map(drop)
        })?;
        self.int("installLocation")?;
        self.bool("isStub")?;
        self.bool("coreApp")?;
        self.bool("requiredForAllUsers")?;
        self.s8s(&[
            "restrictedAccountType",
            "requiredAccountType",
            "overlayTarget",
            "overlayCategory",
        ])?;
        self.int("overlayPriority")?;
        self.bool("overlayIsStatic")?;
        self.int("compileSdkVersion")?;
        self.s8("compileSdkVersionCodename")?;
        if self.r.read_i32()? != 0 {
            self.nested("signingInfo", |d| d.signing_details())?;
        }
        self.bool("isApex")?;
        self.bool("isActiveApex")?;
        self.long("archiveTimeMillis")?;
        if self.r.read_i32()? != 0 {
            self.s8("apexPackageName")?;
        }
        Ok(())
    }

    /// A `ParceledListSlice` with its items inline (the comparison
    /// stitches in those the original fetched later).
    fn list(&mut self, item: fn(&mut D<'_, '_>) -> Result<()>) -> Result<()> {
        let n = self.r.read_i32()?;
        if n <= 0 {
            self.push("items", Value::List(vec![]));
            return Ok(());
        }
        self.s16("creator")?;
        let mut items = Vec::new();
        for _ in 0..n {
            if self.r.read_i32()? == 0 {
                return Err(BAD_VALUE);
            }
            let mut squashed = HashMap::new();
            let mut d = D::new(&mut *self.r, &mut squashed);
            item(&mut d)?;
            items.push(d.done());
        }
        self.push("items", Value::List(items));
        Ok(())
    }
}
