//! Generic passwords of the Mac user's keychain, which the device's lock
//! protects as the Mac's lock: the login keychain is unlocked by the
//! user's login and locked again at logout (or when the user locks it),
//! and a locked keychain answers no item, as a Keychain item of
//! `kSecAttrAccessibleWhenUnlockedThisDeviceOnly` would. Items are of the
//! file-based keychain, never synchronized to other devices; their access
//! lets any application of the user decrypt them, so rebuilding the app
//! host (an ad-hoc signature, a new designated requirement) prompts for
//! nothing, which costs no protection as every process of the user can
//! read the data image anyway. No call shows a prompt: one that would
//! fails with `errSecInteractionNotAllowed`.

use std::ffi::c_void;

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
/// `errSecItemNotFound`.
pub const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

/// An owned (+1) CoreFoundation reference.
struct Owned(CFTypeRef);

impl Owned {
    fn string(s: &str) -> Self {
        // SAFETY: the bytes and their length are the string's.
        Self(unsafe {
            CFStringCreateWithBytes(std::ptr::null(), s.as_ptr(), s.len() as isize, UTF8, false)
        })
    }

    fn data(bytes: &[u8]) -> Self {
        // SAFETY: the bytes and their length are the slice's.
        Self(unsafe { CFDataCreate(std::ptr::null(), bytes.as_ptr(), bytes.len() as isize) })
    }

    fn dictionary() -> Self {
        // SAFETY: the CFType callbacks retain the keys and values set.
        Self(unsafe {
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
    fn item(&self, service: &str, account: &str) -> Owned {
        let query = Owned::dictionary();
        // SAFETY: the constants are CFStringRefs of the framework.
        unsafe {
            query.set(kSecClass, *kSecClassGenericPassword());
            query.set(kSecUseAuthenticationUI, *kSecUseAuthenticationUIFail());
        }
        query.set(kSecAttrService, Owned::string(service).0);
        query.set(kSecAttrAccount, Owned::string(account).0);
        query
    }

    /// The keychain searched, when it is not the user's default.
    fn search_list(&self) -> Option<Owned> {
        // SAFETY: a one-element array of the keychain, retained by it.
        self.0.as_ref().map(|keychain| unsafe {
            Owned(CFArrayCreate(
                std::ptr::null(),
                &keychain.0,
                1,
                kCFTypeArrayCallBacks().cast(),
            ))
        })
    }

    /// The data of the generic password of `service` and `account`, none
    /// when there is no such item; an `OSStatus` otherwise, among them
    /// `errSecInteractionNotAllowed` while the keychain is locked.
    pub fn read(&self, service: &str, account: &str) -> Result<Option<Vec<u8>>, i32> {
        let query = self.item(service, account);
        // SAFETY: the dictionary holds CF objects; a copied result is
        // released as an Owned, read as the CFData asked for.
        unsafe {
            query.set(kSecReturnData, *kCFBooleanTrue());
            query.set(kSecMatchLimit, *kSecMatchLimitOne());
            let list = self.search_list();
            if let Some(list) = &list {
                query.set(kSecMatchSearchList, list.0);
            }
            let mut result = std::ptr::null();
            match SecItemCopyMatching(query.0, &mut result) {
                ERR_SEC_ITEM_NOT_FOUND => Ok(None),
                status => {
                    check(status)?;
                    let data = Owned(result);
                    let len = CFDataGetLength(data.0) as usize;
                    Ok(Some(
                        std::slice::from_raw_parts(CFDataGetBytePtr(data.0), len).to_vec(),
                    ))
                }
            }
        }
    }

    /// Adds the generic password of `service` and `account`, `label`
    /// shown for it, any application of the user allowed to decrypt it;
    /// `errSecDuplicateItem` if there is one.
    pub fn add(&self, service: &str, account: &str, label: &str, data: &[u8]) -> Result<(), i32> {
        let item = self.item(service, account);
        let label = Owned::string(label);
        let data = Owned::data(data);
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
            let access = Owned(access);
            let acls = Owned(SecAccessCopyMatchingACLList(
                access.0,
                *kSecACLAuthorizationDecrypt(),
            ));
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
            fn SecItemDelete(query: CFTypeRef) -> i32;
        }
    }

    /// A keychain of its own in a temporary directory, deleted with the
    /// test: never the user's.
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
            Throwaway(Keychain(Some(Owned(keychain))), dir)
        }
    }

    impl Drop for Throwaway {
        fn drop(&mut self) {
            let keychain = self.0.0.as_ref().unwrap();
            // SAFETY: the keychain this test created.
            assert_eq!(unsafe { SecKeychainDelete(keychain.0) }, 0);
            std::fs::remove_dir_all(&self.1).unwrap();
        }
    }

    /// A password added to a throwaway keychain is read back, found by its
    /// service and account only, refused a second time, then deleted. It
    /// creates and deletes a keychain file, so it runs only when asked.
    #[test]
    #[ignore]
    fn a_password_is_added_read_and_deleted_in_a_throwaway_keychain() {
        const SERVICE: &str = "dev.aim.test.keychain";
        let keychain = Throwaway::new();
        let k = &keychain.0;
        assert_eq!(k.read(SERVICE, "user 0"), Ok(None));
        k.add(SERVICE, "user 0", "aim test", &[7; 32]).unwrap();
        assert_eq!(k.read(SERVICE, "user 0"), Ok(Some(vec![7; 32])));
        assert_eq!(k.read(SERVICE, "user 10"), Ok(None));
        assert_eq!(k.read("dev.aim.test.other", "user 0"), Ok(None));
        // errSecDuplicateItem.
        assert_eq!(k.add(SERVICE, "user 0", "aim test", &[8; 32]), Err(-25299));
        let query = k.item(SERVICE, "user 0");
        let list = k.search_list().unwrap();
        query.set(kSecMatchSearchList, list.0);
        // SAFETY: a query dictionary of ours.
        assert_eq!(unsafe { SecItemDelete(query.0) }, 0);
        assert_eq!(k.read(SERVICE, "user 0"), Ok(None));
    }
}
