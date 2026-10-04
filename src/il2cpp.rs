use std::ffi::{CString, c_char, c_void};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::core::{PCSTR, w};

pub type Ptr = *mut c_void;

#[allow(dead_code)]
pub struct Api {
    domain_get: unsafe extern "C" fn() -> Ptr,
    domain_get_assemblies: unsafe extern "C" fn(Ptr, *mut usize) -> *const Ptr,
    assembly_get_image: unsafe extern "C" fn(Ptr) -> Ptr,
    class_from_name: unsafe extern "C" fn(Ptr, *const c_char, *const c_char) -> Ptr,
    class_get_method_from_name: unsafe extern "C" fn(Ptr, *const c_char, i32) -> Ptr,
    thread_attach: unsafe extern "C" fn(Ptr) -> Ptr,
    thread_detach: unsafe extern "C" fn(Ptr),
    gchandle_new: unsafe extern "C" fn(Ptr, bool) -> u32,
    gchandle_get_target: unsafe extern "C" fn(u32) -> Ptr,
    gchandle_free: unsafe extern "C" fn(u32),
}

static API: OnceLock<Api> = OnceLock::new();

pub fn api() -> &'static Api {
    API.get().expect("il2cpp api not loaded")
}

macro_rules! export {
    ($module:expr, $name:literal) => {{
        let f = unsafe { GetProcAddress($module, PCSTR(concat!($name, "\0").as_ptr())) }
            .ok_or_else(|| anyhow!(concat!("missing export ", $name)))?;
        unsafe { std::mem::transmute(f) }
    }};
}

fn load_api(module: HMODULE) -> Result<Api> {
    Ok(Api {
        domain_get: export!(module, "il2cpp_domain_get"),
        domain_get_assemblies: export!(module, "il2cpp_domain_get_assemblies"),
        assembly_get_image: export!(module, "il2cpp_assembly_get_image"),
        class_from_name: export!(module, "il2cpp_class_from_name"),
        class_get_method_from_name: export!(module, "il2cpp_class_get_method_from_name"),
        thread_attach: export!(module, "il2cpp_thread_attach"),
        thread_detach: export!(module, "il2cpp_thread_detach"),
        gchandle_new: export!(module, "il2cpp_gchandle_new"),
        gchandle_get_target: export!(module, "il2cpp_gchandle_get_target"),
        gchandle_free: export!(module, "il2cpp_gchandle_free"),
    })
}

/// Waits for GameAssembly.dll and the il2cpp domain to come up (we may be injected early).
pub fn wait_for_runtime(timeout: Duration) -> Result<&'static Api> {
    let start = Instant::now();
    loop {
        if let Ok(module) = unsafe { GetModuleHandleW(w!("GameAssembly.dll")) } {
            let api = match API.get() {
                Some(api) => api,
                None => {
                    let _ = API.set(load_api(module)?);
                    api()
                }
            };
            if !unsafe { (api.domain_get)() }.is_null() {
                return Ok(api);
            }
        }
        if start.elapsed() > timeout {
            return Err(anyhow!("timed out waiting for il2cpp runtime"));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Attaches the calling thread to the il2cpp GC for the lifetime of the guard.
pub struct ThreadGuard(Ptr);

impl ThreadGuard {
    pub fn attach() -> Self {
        let api = api();
        ThreadGuard(unsafe { (api.thread_attach)((api.domain_get)()) })
    }
}

impl Drop for ThreadGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { (api().thread_detach)(self.0) }
        }
    }
}

/// Returns the compiled code pointer of `namespace.class::method` with `argc` parameters.
pub fn method_pointer(namespace: &str, class: &str, method: &str, argc: i32) -> Result<usize> {
    let api = api();
    let ns = CString::new(namespace)?;
    let cls = CString::new(class)?;
    let name = CString::new(method)?;
    unsafe {
        let mut count = 0usize;
        let assemblies = (api.domain_get_assemblies)((api.domain_get)(), &mut count);
        if assemblies.is_null() {
            return Err(anyhow!("no assemblies loaded"));
        }
        for i in 0..count {
            let image = (api.assembly_get_image)(*assemblies.add(i));
            let klass = (api.class_from_name)(image, ns.as_ptr(), cls.as_ptr());
            if klass.is_null() {
                continue;
            }
            let info = (api.class_get_method_from_name)(klass, name.as_ptr(), argc);
            if info.is_null() {
                break;
            }
            // MethodInfo::methodPointer is the first field
            let code = *(info as *const usize);
            if code != 0 {
                return Ok(code);
            }
            break;
        }
    }
    Err(anyhow!("couldn't resolve {namespace}.{class}::{method}/{argc}"))
}

/// Strong GC handle so cached managed objects can't be collected out from under us.
pub struct GcHandle(u32);

impl GcHandle {
    pub fn new(obj: Ptr) -> Self {
        GcHandle(unsafe { (api().gchandle_new)(obj, false) })
    }

    pub fn target(&self) -> Ptr {
        unsafe { (api().gchandle_get_target)(self.0) }
    }
}

impl Drop for GcHandle {
    fn drop(&mut self) {
        unsafe { (api().gchandle_free)(self.0) }
    }
}

/// UnityEngine.Object is alive while its native m_CachedPtr (offset 0x10) is non-null.
pub unsafe fn unity_alive(obj: Ptr) -> bool {
    !obj.is_null() && unsafe { *(obj as *const usize).add(2) } != 0
}

/// Copies a System.String's UTF-16 contents (length at 0x10, chars at 0x14).
pub unsafe fn string_key(s: Ptr) -> Vec<u16> {
    unsafe {
        let len = *((s as *const u8).add(0x10) as *const i32);
        let chars = (s as *const u8).add(0x14) as *const u16;
        std::slice::from_raw_parts(chars, len.max(0) as usize).to_vec()
    }
}
