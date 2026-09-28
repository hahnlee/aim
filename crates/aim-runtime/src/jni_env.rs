//! Thin JNI helpers for natives this crate registers on framework and
//! services classes. They mirror libnativehelper (`jniThrowIOException`,
//! `jniGetFDFromFileDescriptor`, ...) so ported natives keep their AOSP shape.

use jni_sys::{
    JNI_OK, JNIEnv, JNINativeMethod, jclass, jfieldID, jint, jlong, jmethodID, jobject, jstring,
    jvalue,
};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::OnceLock;

/// A JNIEnv borrowed for one native call.
#[derive(Clone, Copy)]
pub struct Env(*mut JNIEnv);

macro_rules! call {
    ($env:expr, $name:ident $(, $arg:expr)*) => {{
        let env = $env.0;
        // SAFETY: `Env` only wraps the JNIEnv of the current native frame,
        // whose function table is complete.
        unsafe { ((**env).$name.expect(stringify!($name)))(env $(, $arg)*) }
    }};
}

impl Env {
    /// # Safety
    /// `env` must be the calling thread's JNIEnv.
    pub unsafe fn new(env: *mut JNIEnv) -> Self {
        Self(env)
    }

    pub fn exception_check(self) -> bool {
        call!(self, ExceptionCheck) != 0
    }

    pub fn find_class(self, name: &CStr) -> jclass {
        call!(self, FindClass, name.as_ptr())
    }

    /// `loader.loadClass(name)` with a binary (dotted) name, for classes of
    /// a non-boot class loader such as the system server's services.jar.
    pub fn load_class(self, loader: jobject, load: jmethodID, name: &str) -> jclass {
        let Ok(name) = CString::new(name) else {
            return std::ptr::null_mut();
        };
        let name = call!(self, NewStringUTF, name.as_ptr());
        if name.is_null() {
            return std::ptr::null_mut();
        }
        let arguments = [jvalue { l: name }];
        let class = call!(self, CallObjectMethodA, loader, load, arguments.as_ptr());
        self.delete_local_ref(name);
        if self.exception_check() {
            return std::ptr::null_mut();
        }
        class
    }

    /// The raw JNIEnv, for C helpers that take it.
    pub fn raw(self) -> *mut JNIEnv {
        self.0
    }

    pub fn exception_clear(self) {
        call!(self, ExceptionClear)
    }

    pub fn static_method_id(self, class: jclass, name: &CStr, signature: &CStr) -> jmethodID {
        call!(
            self,
            GetStaticMethodID,
            class,
            name.as_ptr(),
            signature.as_ptr()
        )
    }

    pub fn call_static_object(
        self,
        class: jclass,
        method: jmethodID,
        arguments: &[jvalue],
    ) -> jobject {
        call!(
            self,
            CallStaticObjectMethodA,
            class,
            method,
            arguments.as_ptr()
        )
    }

    /// A Java string from UTF-8 text (interior NULs end it).
    pub fn new_string_utf(self, text: &str) -> jstring {
        let text = CString::new(text.split('\0').next().unwrap_or("")).unwrap_or_default();
        call!(self, NewStringUTF, text.as_ptr())
    }

    pub fn delete_local_ref(self, object: jobject) {
        if !object.is_null() {
            call!(self, DeleteLocalRef, object);
        }
    }

    pub fn new_global_ref(self, object: jobject) -> jobject {
        call!(self, NewGlobalRef, object)
    }

    pub fn field_id(self, class: jclass, name: &CStr, signature: &CStr) -> jfieldID {
        call!(self, GetFieldID, class, name.as_ptr(), signature.as_ptr())
    }

    pub fn method_id(self, class: jclass, name: &CStr, signature: &CStr) -> jmethodID {
        call!(self, GetMethodID, class, name.as_ptr(), signature.as_ptr())
    }

    pub fn register_natives(self, class: jclass, methods: &[JNINativeMethod]) -> bool {
        !class.is_null()
            && call!(
                self,
                RegisterNatives,
                class,
                methods.as_ptr(),
                methods.len() as jint
            ) == JNI_OK
    }

    pub fn throw(self, class: &CStr, message: &str) {
        let class = self.find_class(class);
        if class.is_null() {
            return;
        }
        let message = CString::new(message).unwrap_or_default();
        call!(self, ThrowNew, class, message.as_ptr());
        self.delete_local_ref(class);
    }

    pub fn throw_null_pointer(self) {
        self.throw(c"java/lang/NullPointerException", "");
    }

    /// libnativehelper `jniThrowIOException`: the message is strerror(errno).
    pub fn throw_io(self, errno: i32) {
        let message = std::io::Error::from_raw_os_error(errno).to_string();
        self.throw(c"java/io/IOException", &message);
    }

    pub fn string(self, value: jstring) -> Option<String> {
        if value.is_null() {
            return None;
        }
        let chars = call!(self, GetStringUTFChars, value, std::ptr::null_mut());
        if chars.is_null() {
            return None;
        }
        // SAFETY: GetStringUTFChars returns a NUL-terminated buffer that stays
        // valid until released below.
        let text = unsafe { CStr::from_ptr(chars) }
            .to_string_lossy()
            .into_owned();
        call!(self, ReleaseStringUTFChars, value, chars);
        Some(text)
    }

    pub fn new_string(self, text: &CStr) -> jstring {
        call!(self, NewStringUTF, text.as_ptr())
    }

    pub fn object_class(self, object: jobject) -> jclass {
        call!(self, GetObjectClass, object)
    }

    pub fn call_void(self, object: jobject, method: jmethodID, arguments: &[jvalue]) {
        call!(self, CallVoidMethodA, object, method, arguments.as_ptr())
    }

    pub fn set_long_array_region(self, array: jobject, start: jint, input: &[jlong]) {
        call!(
            self,
            SetLongArrayRegion,
            array,
            start,
            input.len() as jint,
            input.as_ptr()
        )
    }

    /// libnativehelper jniGetFDFromFileDescriptor (`FileDescriptor.descriptor`).
    pub fn file_descriptor(self, descriptor: jobject) -> jint {
        static FIELD: OnceLock<usize> = OnceLock::new();
        let field = *FIELD.get_or_init(|| {
            let class = self.find_class(c"java/io/FileDescriptor");
            let field = self.field_id(class, c"descriptor", c"I");
            self.delete_local_ref(class);
            field as usize
        });
        if field == 0 {
            return -1;
        }
        self.int_field(descriptor, field as jfieldID)
    }

    pub fn new_long_array(self, length: jint) -> jobject {
        call!(self, NewLongArray, length)
    }

    pub fn new_int_array(self, length: jint) -> jobject {
        call!(self, NewIntArray, length)
    }

    pub fn throw_object(self, throwable: jobject) {
        call!(self, Throw, throwable);
    }

    pub fn array_length(self, array: jobject) -> jint {
        call!(self, GetArrayLength, array)
    }

    pub fn object_field(self, object: jobject, field: jfieldID) -> jobject {
        call!(self, GetObjectField, object, field)
    }

    pub fn set_object_field(self, object: jobject, field: jfieldID, value: jobject) {
        call!(self, SetObjectField, object, field, value)
    }

    pub fn int_field(self, object: jobject, field: jfieldID) -> jint {
        call!(self, GetIntField, object, field)
    }

    pub fn set_int_field(self, object: jobject, field: jfieldID, value: jint) {
        call!(self, SetIntField, object, field, value)
    }

    pub fn new_object(
        self,
        class: jclass,
        constructor: jmethodID,
        arguments: &[jvalue],
    ) -> jobject {
        call!(self, NewObjectA, class, constructor, arguments.as_ptr())
    }

    pub fn new_object_array(self, length: jint, class: jclass) -> jobject {
        call!(self, NewObjectArray, length, class, std::ptr::null_mut())
    }

    pub fn object_array_element(self, array: jobject, index: jint) -> jobject {
        call!(self, GetObjectArrayElement, array, index)
    }

    pub fn set_object_array_element(self, array: jobject, index: jint, value: jobject) {
        call!(self, SetObjectArrayElement, array, index, value)
    }

    pub fn byte_array_region(self, array: jobject, start: jint, output: &mut [u8]) {
        call!(
            self,
            GetByteArrayRegion,
            array,
            start,
            output.len() as jint,
            output.as_mut_ptr().cast()
        )
    }

    pub fn set_byte_array_region(self, array: jobject, start: jint, input: &[u8]) {
        call!(
            self,
            SetByteArrayRegion,
            array,
            start,
            input.len() as jint,
            input.as_ptr().cast()
        )
    }
}

/// A `JNINativeMethod` entry from static name and signature strings.
pub fn native(
    name: &'static CStr,
    signature: &'static CStr,
    function: *mut std::ffi::c_void,
) -> JNINativeMethod {
    JNINativeMethod {
        name: name.as_ptr() as *mut c_char,
        signature: signature.as_ptr() as *mut c_char,
        fnPtr: function,
    }
}
