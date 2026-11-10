//! Generic passwords of the Mac user's keychain, which the device's lock
//! protects as the Mac's lock: the login keychain is unlocked by the
//! user's login and locked again at logout (or when the user locks it),
//! and a locked keychain answers no item, as a Keychain item of
//! `kSecAttrAccessibleWhenUnlockedThisDeviceOnly` would. Items are of the
//! file-based keychain, never synchronized to other devices; their access
//! lets any application of the user decrypt them, so rebuilding the app
//! host (an ad-hoc signature, a new designated requirement) prompts for
//! nothing, which costs no protection as every process of the user can
//! read the data image anyway. An item made with the default access
//! instead is bound to its maker's code (its partition list): another
//! ad-hoc build reading it blocks on a prompt. No call shows a prompt: a
//! locked keychain fails the read (`errAuthorizationInternal`).
//!
//! A throwaway data directory (a test's, the build's) keeps its items in a
//! keychain file of its own instead ([`Keychain::file`]), out of the
//! user's search list and removed with the directory, so that nothing of
//! it reaches the user's keychain.

use std::ffi::{CStr, CString, OsStr, c_char, c_void};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use zeroize::Zeroizing;

type CFTypeRef = *const c_void;

aim_hostcall::dylib! {
    static CORE_FOUNDATION = c"/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation" {
        static kCFTypeDictionaryKeyCallBacks: c_void;
        static kCFTypeDictionaryValueCallBacks: c_void;
        static kCFTypeArrayCallBacks: c_void;
        static kCFBooleanTrue: CFTypeRef;
        fn CFRelease(cf: CFTypeRef);
        fn CFStringCreateWithBytes(
            alloc: CFTypeRef,
            bytes: *const u8,
            len: isize,
            encoding: u32,
            external: bool,
        ) -> CFTypeRef;
        fn CFDataCreate(alloc: CFTypeRef, bytes: *const u8, len: isize) -> CFTypeRef;
        fn CFDataGetBytePtr(data: CFTypeRef) -> *const u8;
        fn CFDataGetLength(data: CFTypeRef) -> isize;
        fn CFArrayCreate(
            alloc: CFTypeRef,
            values: *const CFTypeRef,
            count: isize,
            callbacks: *const c_void,
        ) -> CFTypeRef;
        fn CFArrayGetCount(array: CFTypeRef) -> isize;
        fn CFArrayGetValueAtIndex(array: CFTypeRef, index: isize) -> CFTypeRef;
        fn CFArrayCreateMutable(alloc: CFTypeRef, capacity: isize, callbacks: *const c_void) -> CFTypeRef;
        fn CFArrayAppendValue(array: CFTypeRef, value: CFTypeRef);
        fn CFEqual(a: CFTypeRef, b: CFTypeRef) -> bool;
        fn CFDictionaryCreateMutable(
            alloc: CFTypeRef,
            capacity: isize,
            keys: *const c_void,
            values: *const c_void,
        ) -> CFTypeRef;
        fn CFDictionarySetValue(dict: CFTypeRef, key: CFTypeRef, value: CFTypeRef);
    }
}

aim_hostcall::dylib! {
    static SECURITY = c"/System/Library/Frameworks/Security.framework/Security" {
        static kSecClass: CFTypeRef;
        static kSecClassGenericPassword: CFTypeRef;
        static kSecAttrService: CFTypeRef;
        static kSecAttrAccount: CFTypeRef;
        static kSecAttrLabel: CFTypeRef;
        static kSecAttrAccess: CFTypeRef;
        static kSecValueData: CFTypeRef;
        static kSecReturnData: CFTypeRef;
        static kSecMatchLimit: CFTypeRef;
        static kSecMatchLimitOne: CFTypeRef;
        static kSecMatchSearchList: CFTypeRef;
        static kSecUseKeychain: CFTypeRef;
        static kSecUseAuthenticationUI: CFTypeRef;
        static kSecUseAuthenticationUIFail: CFTypeRef;
        static kSecACLAuthorizationDecrypt: CFTypeRef;
        fn SecItemAdd(attributes: CFTypeRef, result: *mut CFTypeRef) -> i32;
        fn SecItemCopyMatching(query: CFTypeRef, result: *mut CFTypeRef) -> i32;
        fn SecAccessCreate(descriptor: CFTypeRef, trusted: CFTypeRef, access: *mut CFTypeRef) -> i32;
        fn SecAccessCopyMatchingACLList(access: CFTypeRef, authorization: CFTypeRef) -> CFTypeRef;
        fn SecACLSetContents(
            acl: CFTypeRef,
            applications: CFTypeRef,
            description: CFTypeRef,
            prompt: u16,
        ) -> i32;
        fn SecKeychainOpen(path: *const c_char, keychain: *mut CFTypeRef) -> i32;
        fn SecKeychainCreate(
            path: *const c_char,
            password_len: u32,
            password: *const c_void,
            prompt: bool,
            access: CFTypeRef,
            keychain: *mut CFTypeRef,
        ) -> i32;
        fn SecKeychainUnlock(
            keychain: CFTypeRef,
            password_len: u32,
            password: *const c_void,
            use_password: bool,
        ) -> i32;
        fn SecKeychainCopyDomainSearchList(domain: i32, list: *mut CFTypeRef) -> i32;
        fn SecKeychainSetDomainSearchList(domain: i32, list: CFTypeRef) -> i32;
    }
}

/// `kCFStringEncodingUTF8`.
const UTF8: u32 = 0x0800_0100;
/// `errSecItemNotFound`, `errSecAllocate`, `errSecParam`, `errSecIO`.
pub const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;
const ERR_SEC_ALLOCATE: i32 = -108;
const ERR_SEC_PARAM: i32 = -50;
const ERR_SEC_IO: i32 = -36;
/// `kSecPreferencesDomainUser`.
const DOMAIN_USER: i32 = 0;
/// The password of a keychain file: it guards nothing from the user, who
/// can read the data image it belongs to anyway.
const FILE_PASSWORD: &[u8] = b"aim";

/// An owned (+1) CoreFoundation reference, never NULL.
struct Owned(CFTypeRef);

impl Owned {
    /// `r`, which a CoreFoundation or Security call made: none (NULL) is
    /// `errSecAllocate`.
    fn new(r: CFTypeRef) -> Result<Self, i32> {
        if r.is_null() {
            Err(ERR_SEC_ALLOCATE)
        } else {
            Ok(Self(r))
        }
    }

    fn string(s: &str) -> Result<Self, i32> {
        // SAFETY: the bytes and their length are the string's.
        Self::new(unsafe {
            CFStringCreateWithBytes(std::ptr::null(), s.as_ptr(), s.len() as isize, UTF8, false)
        })
    }

    fn data(bytes: &[u8]) -> Result<Self, i32> {
        // SAFETY: the bytes and their length are the slice's.
        Self::new(unsafe { CFDataCreate(std::ptr::null(), bytes.as_ptr(), bytes.len() as isize) })
    }

    fn dictionary() -> Result<Self, i32> {
        // SAFETY: the CFType callbacks retain the keys and values set.
        Self::new(unsafe {
            CFDictionaryCreateMutable(
                std::ptr::null(),
                0,
                kCFTypeDictionaryKeyCallBacks().cast(),
                kCFTypeDictionaryValueCallBacks().cast(),
            )
        })
    }

    /// `dict[*key] = value`, of a key the Security framework exports.
    fn set(&self, key: fn() -> *const CFTypeRef, value: CFTypeRef) {
        // SAFETY: a mutable dictionary of ours; the key symbol is a
        // CFStringRef, the value a CF object.
        unsafe { CFDictionarySetValue(self.0, *key(), value) }
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: we hold one reference.
        unsafe { CFRelease(self.0) };
    }
}

fn check(status: i32) -> Result<(), i32> {
    if status == 0 { Ok(()) } else { Err(status) }
}

/// A keychain: the user's default one (their login keychain), or one
/// opened by reference.
pub struct Keychain(Option<Owned>);

impl Keychain {
    /// The Mac user's default keychain.
    pub fn user() -> Self {
        Keychain(None)
    }

    /// The keychain file `path`, made if there is none, unlocked, and
    /// taken out of the user's search list, where creating it puts it:
    /// each open does, so a file whose maker was stopped before is too.
    pub fn file(path: &Path) -> Result<Self, i32> {
        let name = CString::new(path.as_os_str().as_bytes()).map_err(|_| ERR_SEC_PARAM)?;
        let (password, len) = (FILE_PASSWORD.as_ptr().cast(), FILE_PASSWORD.len() as u32);
        let _search_list = SearchListLock::take()?;
        let created = !path.exists();
        let mut keychain = std::ptr::null();
        // SAFETY: a NUL-terminated path, the password's bytes; the keychain
        // made or opened is released as an Owned.
        let keychain = unsafe {
            check(if created {
                SecKeychainCreate(
                    name.as_ptr(),
                    len,
                    password,
                    false,
                    std::ptr::null(),
                    &mut keychain,
                )
            } else {
                SecKeychainOpen(name.as_ptr(), &mut keychain)
            })?;
            Owned::new(keychain)?
        };
        unlist(keychain.0)?;
        // SAFETY: the keychain and the password's bytes.
        check(unsafe { SecKeychainUnlock(keychain.0, len, password, true) })?;
        Ok(Keychain(Some(keychain)))
    }

    /// The item of `service` and `account`, without its data.
    fn item(&self, service: &str, account: &str) -> Result<Owned, i32> {
        let query = Owned::dictionary()?;
        // SAFETY: the constants are CFStringRefs of the framework.
        unsafe {
            query.set(kSecClass, *kSecClassGenericPassword());
            query.set(kSecUseAuthenticationUI, *kSecUseAuthenticationUIFail());
        }
        query.set(kSecAttrService, Owned::string(service)?.0);
        query.set(kSecAttrAccount, Owned::string(account)?.0);
        Ok(query)
    }

    /// The keychain searched, when it is not the user's default.
    fn search_list(&self) -> Result<Option<Owned>, i32> {
        self.0
            .as_ref()
            .map(|keychain| {
                // SAFETY: a one-element array of the keychain, retained by it.
                Owned::new(unsafe {
                    CFArrayCreate(
                        std::ptr::null(),
                        &keychain.0,
                        1,
                        kCFTypeArrayCallBacks().cast(),
                    )
                })
            })
            .transpose()
    }

    /// The data of the generic password of `service` and `account`, none
    /// when there is no such item; an `OSStatus` otherwise, among them
    /// `errAuthorizationInternal` while the keychain is locked.
    pub fn read(&self, service: &str, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>, i32> {
        let query = self.item(service, account)?;
        let list = self.search_list()?;
        if let Some(list) = &list {
            query.set(kSecMatchSearchList, list.0);
        }
        // SAFETY: the dictionary holds CF objects; a copied result is
        // released as an Owned, read as the CFData asked for.
        unsafe {
            query.set(kSecReturnData, *kCFBooleanTrue());
            query.set(kSecMatchLimit, *kSecMatchLimitOne());
            let mut result = std::ptr::null();
            match SecItemCopyMatching(query.0, &mut result) {
                ERR_SEC_ITEM_NOT_FOUND => Ok(None),
                status => {
                    check(status)?;
                    let data = Owned::new(result)?;
                    let len = CFDataGetLength(data.0) as usize;
                    Ok(Some(Zeroizing::new(
                        std::slice::from_raw_parts(CFDataGetBytePtr(data.0), len).to_vec(),
                    )))
                }
            }
        }
    }

    /// Adds the generic password of `service` and `account`, `label`
    /// shown for it, any application of the user allowed to decrypt it;
    /// `errSecDuplicateItem` if there is one.
    pub fn add(&self, service: &str, account: &str, label: &str, data: &[u8]) -> Result<(), i32> {
        let item = self.item(service, account)?;
        let label = Owned::string(label)?;
        let data = Owned::data(data)?;
        item.set(kSecAttrLabel, label.0);
        item.set(kSecValueData, data.0);
        if let Some(keychain) = &self.0 {
            item.set(kSecUseKeychain, keychain.0);
        }
        // SAFETY: `SecAccessCreate` makes an access trusting the caller;
        // each of its decrypt ACLs is given no application list, which is
        // any application. The ACL list and access are released after the
        // item retained the access.
        unsafe {
            let mut access = std::ptr::null();
            check(SecAccessCreate(label.0, std::ptr::null(), &mut access))?;
            let access = Owned::new(access)?;
            let acls = Owned::new(SecAccessCopyMatchingACLList(
                access.0,
                *kSecACLAuthorizationDecrypt(),
            ))?;
            for i in 0..CFArrayGetCount(acls.0) {
                let acl = CFArrayGetValueAtIndex(acls.0, i);
                check(SecACLSetContents(acl, std::ptr::null(), label.0, 0))?;
            }
            item.set(kSecAttrAccess, access.0);
            check(SecItemAdd(item.0, std::ptr::null_mut()))
        }
    }
}

/// The user's search list is read and written back whole, by
/// `SecKeychainCreate` as by [`unlist`]: an exclusive `flock` on a lock
/// file of the user's makes aim's processes do it one at a time, so none
/// loses another's change. Closing the file releases it.
struct SearchListLock {
    _file: std::fs::File,
}

impl SearchListLock {
    fn take() -> Result<Self, i32> {
        let mut dir = [0 as c_char; libc::PATH_MAX as usize];
        // SAFETY: the buffer and its length.
        let n =
            unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, dir.as_mut_ptr(), dir.len()) };
        if n == 0 || n > dir.len() {
            return Err(ERR_SEC_IO);
        }
        // SAFETY: confstr NUL-terminated what it wrote.
        let dir = unsafe { CStr::from_ptr(dir.as_ptr()) };
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(
                Path::new(OsStr::from_bytes(dir.to_bytes()))
                    .join("dev.aim.keychain-search-list.lock"),
            )
            .map_err(|_| ERR_SEC_IO)?;
        loop {
            // SAFETY: an open file of ours.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
                return Ok(Self { _file: file });
            }
            if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                return Err(ERR_SEC_IO);
            }
        }
    }
}

/// Takes `keychain` out of the user's keychain search list if it is
/// there, keeping every other entry.
fn unlist(keychain: CFTypeRef) -> Result<(), i32> {
    // SAFETY: the copied list and the filtered one are released as Owned;
    // the list holds SecKeychainRefs.
    unsafe {
        let mut list = std::ptr::null();
        check(SecKeychainCopyDomainSearchList(DOMAIN_USER, &mut list))?;
        let list = Owned::new(list)?;
        let kept = Owned::new(CFArrayCreateMutable(
            std::ptr::null(),
            0,
            kCFTypeArrayCallBacks().cast(),
        ))?;
        let count = CFArrayGetCount(list.0);
        for i in 0..count {
            let entry = CFArrayGetValueAtIndex(list.0, i);
            if !CFEqual(entry, keychain) {
                CFArrayAppendValue(kept.0, entry);
            }
        }
        if CFArrayGetCount(kept.0) != count {
            check(SecKeychainSetDomainSearchList(DOMAIN_USER, kept.0))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    aim_hostcall::dylib! {
        static TEST_SECURITY = c"/System/Library/Frameworks/Security.framework/Security" {
            fn SecKeychainDelete(keychain: CFTypeRef) -> i32;
            fn SecKeychainLock(keychain: CFTypeRef) -> i32;
            fn SecItemDelete(query: CFTypeRef) -> i32;
        }
    }

    aim_hostcall::dylib! {
        static TEST_CORE_FOUNDATION = c"/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation" {
            fn CFArrayCreateMutableCopy(alloc: CFTypeRef, capacity: isize, array: CFTypeRef) -> CFTypeRef;
        }
    }

    /// The user's search list.
    fn search_list() -> Owned {
        let mut list = std::ptr::null();
        // SAFETY: the copied list is released as an Owned.
        check(unsafe { SecKeychainCopyDomainSearchList(DOMAIN_USER, &mut list) }).unwrap();
        Owned::new(list).unwrap()
    }

    /// Whether the user's search list holds the keychain file `path`.
    fn listed(path: &Path) -> bool {
        let name = CString::new(path.as_os_str().as_bytes()).unwrap();
        let mut keychain = std::ptr::null();
        // SAFETY: a NUL-terminated path; the reference is released as an
        // Owned, the list's entries compared with it.
        unsafe {
            check(SecKeychainOpen(name.as_ptr(), &mut keychain)).unwrap();
            let keychain = Owned::new(keychain).unwrap();
            let list = search_list();
            (0..CFArrayGetCount(list.0))
                .any(|i| CFEqual(CFArrayGetValueAtIndex(list.0, i), keychain.0))
        }
    }

    /// Keychain files made at once stay out of the user's search list and
    /// take nothing else out of it; one its maker left there (stopped
    /// before taking it out) is taken out when it is opened again. It
    /// creates and deletes keychain files, so it runs only when asked.
    #[test]
    #[ignore]
    fn keychain_files_stay_out_of_the_search_list() {
        let dir = std::env::temp_dir().join(format!("aim-keychain-list-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let paths: Vec<_> = (0..4).map(|i| dir.join(format!("{i}.keychain"))).collect();
        // SAFETY: the user's list, read for its length.
        let before = unsafe { CFArrayGetCount(search_list().0) };
        std::thread::scope(|scope| {
            for path in &paths {
                scope.spawn(|| drop(Keychain::file(path).unwrap()));
            }
        });
        assert!(paths.iter().all(|path| !listed(path)));
        // SAFETY: as above.
        assert_eq!(unsafe { CFArrayGetCount(search_list().0) }, before);
        // Back in the list, as SecKeychainCreate puts it there.
        let left = Keychain::file(&paths[0]).unwrap();
        {
            let _lock = SearchListLock::take().unwrap();
            let list = search_list();
            // SAFETY: a mutable copy of the list, the keychain appended.
            unsafe {
                let list =
                    Owned::new(CFArrayCreateMutableCopy(std::ptr::null(), 0, list.0)).unwrap();
                CFArrayAppendValue(list.0, left.0.as_ref().unwrap().0);
                check(SecKeychainSetDomainSearchList(DOMAIN_USER, list.0)).unwrap();
            }
        }
        assert!(listed(&paths[0]));
        drop(Keychain::file(&paths[0]).unwrap());
        assert!(!listed(&paths[0]));
        for path in &paths {
            let keychain = Keychain::file(path).unwrap();
            // SAFETY: a keychain this test created.
            assert_eq!(
                unsafe { SecKeychainDelete(keychain.0.as_ref().unwrap().0) },
                0
            );
        }
        // SAFETY: as above.
        assert_eq!(unsafe { CFArrayGetCount(search_list().0) }, before);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// `errAuthorizationInternal`, `errSecDuplicateItem`.
    const ERR_AUTHORIZATION_INTERNAL: i32 = -60008;
    const ERR_SEC_DUPLICATE_ITEM: i32 = -25299;

    /// A keychain file in a temporary directory, deleted with the test
    /// however it ends: never the user's.
    struct Throwaway(Keychain, std::path::PathBuf);

    impl Throwaway {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("aim-keychain-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let keychain = Keychain::file(&dir.join("test.keychain")).unwrap();
            Throwaway(keychain, dir)
        }

        fn keychain(&self) -> CFTypeRef {
            self.0.0.as_ref().unwrap().0
        }
    }

    impl Drop for Throwaway {
        fn drop(&mut self) {
            // SAFETY: the keychain this test created.
            let status = unsafe { SecKeychainDelete(self.keychain()) };
            if status != 0 {
                eprintln!("SecKeychainDelete: OSStatus {status}");
            }
            if let Err(e) = std::fs::remove_dir_all(&self.1) {
                eprintln!("{}: {e}", self.1.display());
            }
        }
    }

    /// A password added to a throwaway keychain is read back, found by its
    /// service and account only, refused a second time and deleted; once the keychain is locked, it
    /// is refused without a prompt. It creates and
    /// deletes a keychain file, so it runs only when asked.
    #[test]
    #[ignore]
    fn a_password_is_added_read_and_deleted_in_a_throwaway_keychain() {
        const SERVICE: &str = "dev.aim.test.keychain";
        let keychain = Throwaway::new();
        let k = &keychain.0;
        let read = |service, account| k.read(service, account).map(|d| d.map(|d| d.to_vec()));
        assert_eq!(read(SERVICE, "user 0"), Ok(None));
        k.add(SERVICE, "user 0", "aim test", &[7; 32]).unwrap();
        assert_eq!(read(SERVICE, "user 0"), Ok(Some(vec![7; 32])));
        assert_eq!(read(SERVICE, "user 10"), Ok(None));
        assert_eq!(read("dev.aim.test.other", "user 0"), Ok(None));
        assert_eq!(
            k.add(SERVICE, "user 0", "aim test", &[8; 32]),
            Err(ERR_SEC_DUPLICATE_ITEM)
        );
        let query = k.item(SERVICE, "user 0").unwrap();
        let list = k.search_list().unwrap().unwrap();
        query.set(kSecMatchSearchList, list.0);
        // SAFETY: a query dictionary of ours.
        assert_eq!(unsafe { SecItemDelete(query.0) }, 0);
        assert_eq!(read(SERVICE, "user 0"), Ok(None));
        k.add(SERVICE, "user 0", "aim test", &[9; 32]).unwrap();
        // SAFETY: the throwaway keychain.
        assert_eq!(unsafe { SecKeychainLock(keychain.keychain()) }, 0);
        assert_eq!(read(SERVICE, "user 0"), Err(ERR_AUTHORIZATION_INTERNAL));
        // Opened again, as the next boot of its data directory opens it:
        // unlocked, with the item kept.
        let again = Keychain::file(&keychain.1.join("test.keychain")).unwrap();
        assert_eq!(
            again.read(SERVICE, "user 0").map(|d| d.map(|d| d.to_vec())),
            Ok(Some(vec![9; 32]))
        );
    }
}
