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

use std::ffi::c_void;

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
    }
}

/// `kCFStringEncodingUTF8`.
const UTF8: u32 = 0x0800_0100;
/// `errSecItemNotFound`, `errSecAllocate`.
pub const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;
const ERR_SEC_ALLOCATE: i32 = -108;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{CString, c_char};
    use std::os::unix::ffi::OsStrExt;

    aim_hostcall::dylib! {
        static TEST_SECURITY = c"/System/Library/Frameworks/Security.framework/Security" {
            fn SecKeychainCreate(
                path: *const c_char,
                password_len: u32,
                password: *const c_void,
                prompt: bool,
                access: CFTypeRef,
                keychain: *mut CFTypeRef,
            ) -> i32;
            fn SecKeychainDelete(keychain: CFTypeRef) -> i32;
            fn SecKeychainLock(keychain: CFTypeRef) -> i32;
            fn SecKeychainCopyDomainSearchList(domain: i32, list: *mut CFTypeRef) -> i32;
            fn SecKeychainSetDomainSearchList(domain: i32, list: CFTypeRef) -> i32;
            fn SecItemDelete(query: CFTypeRef) -> i32;
        }
    }

    aim_hostcall::dylib! {
        static TEST_CORE_FOUNDATION = c"/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation" {
            fn CFEqual(a: CFTypeRef, b: CFTypeRef) -> bool;
            fn CFArrayCreateMutable(alloc: CFTypeRef, capacity: isize, callbacks: *const c_void) -> CFTypeRef;
            fn CFArrayAppendValue(array: CFTypeRef, value: CFTypeRef);
        }
    }

    /// `kSecPreferencesDomainUser`, `errAuthorizationInternal`,
    /// `errSecDuplicateItem`.
    const DOMAIN_USER: i32 = 0;
    const ERR_AUTHORIZATION_INTERNAL: i32 = -60008;
    const ERR_SEC_DUPLICATE_ITEM: i32 = -25299;

    /// Takes `keychain` out of the user's keychain search list, if
    /// creating it put it there.
    fn unlist(keychain: CFTypeRef) {
        // SAFETY: the copied list and the filtered one are released as
        // Owned; the list holds SecKeychainRefs.
        unsafe {
            let mut list = std::ptr::null();
            assert_eq!(SecKeychainCopyDomainSearchList(DOMAIN_USER, &mut list), 0);
            let list = Owned::new(list).unwrap();
            let kept = Owned::new(CFArrayCreateMutable(
                std::ptr::null(),
                0,
                kCFTypeArrayCallBacks().cast(),
            ))
            .unwrap();
            let count = CFArrayGetCount(list.0);
            for i in 0..count {
                let entry = CFArrayGetValueAtIndex(list.0, i);
                if !CFEqual(entry, keychain) {
                    CFArrayAppendValue(kept.0, entry);
                }
            }
            if CFArrayGetCount(kept.0) != count {
                assert_eq!(SecKeychainSetDomainSearchList(DOMAIN_USER, kept.0), 0);
            }
        }
    }

    /// A keychain of its own in a temporary directory, never in the
    /// user's search list, deleted with the test however it ends: never
    /// the user's.
    struct Throwaway(Keychain, std::path::PathBuf);

    impl Throwaway {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("aim-keychain-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = CString::new(dir.join("test.keychain").as_os_str().as_bytes()).unwrap();
            let password = b"aim";
            let mut keychain = std::ptr::null();
            // SAFETY: a NUL-terminated path, the password's bytes.
            let status = unsafe {
                SecKeychainCreate(
                    path.as_ptr(),
                    password.len() as u32,
                    password.as_ptr().cast(),
                    false,
                    std::ptr::null(),
                    &mut keychain,
                )
            };
            assert_eq!(status, 0);
            let throwaway = Throwaway(Keychain(Some(Owned::new(keychain).unwrap())), dir);
            unlist(keychain);
            throwaway
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
    }
}
