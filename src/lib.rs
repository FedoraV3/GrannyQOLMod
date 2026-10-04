use std::ffi::c_void;
use windows::Win32::Foundation::HINSTANCE;
use windows::Win32::System::Threading::{CreateThread, THREAD_CREATION_FLAGS};

mod hooks;
mod il2cpp;
unsafe extern "system" fn qolthread(arg: *mut c_void) -> u32 {
    // a panic here would abort the game, so just leave it unpatched on failure
    let overlay = hooks::overlay::init(HINSTANCE(arg));
    let perf = hooks::perf::install();
    (overlay.is_ok() && perf.is_ok()) as u32
}
#[unsafe(no_mangle)]
#[allow(non_snake_case)]
unsafe extern "system" fn DllMain(
    hinst_dll: HINSTANCE,
    fdw_reason: u32,
    _lpv_reserved: *mut c_void
) -> u32 {
    match fdw_reason {
        1 => {
            unsafe {
                let _ = CreateThread(
                    None,
                    0,
                    Some(qolthread),
                    Some(hinst_dll.0 as *const c_void),
                    THREAD_CREATION_FLAGS(0),
                    None
                );
            }
        }
        _ => ()
    }


    1
}
