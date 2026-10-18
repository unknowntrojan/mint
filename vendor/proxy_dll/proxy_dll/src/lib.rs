pub use proxy_dll_proc::proxy_dll;
pub use windows_sys;

#[cfg(windows)]
pub fn get_system_directory() -> std::io::Result<std::path::PathBuf> {
    use windows_sys::Win32::{
        Foundation::MAX_PATH, System::SystemInformation::GetSystemDirectoryW,
    };

    let mut path = [0u16; MAX_PATH as usize];

    let len = unsafe { GetSystemDirectoryW(path.as_mut_ptr(), path.len() as u32) as usize };
    if len == 0 {
        return Err(std::io::Error::last_os_error());
    }

    use std::os::windows::ffi::OsStringExt;
    let os_str = std::ffi::OsString::from_wide(&path[..len]);
    Ok(std::path::PathBuf::from(&os_str))
}
