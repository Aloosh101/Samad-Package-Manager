use std::ffi::CStr;
use nix::libc::{uname, utsname};

use crate::error::SpmResult;

pub fn get_kernel_info() -> SpmResult<String> {
    let mut uts = unsafe { std::mem::zeroed::<utsname>() };
    unsafe { uname(&mut uts) };
    let release = unsafe { CStr::from_ptr(uts.release.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    Ok(release)
}

pub fn host_arch() -> String {
    let mut uts = unsafe { std::mem::zeroed::<utsname>() };
    unsafe { uname(&mut uts) };
    unsafe { CStr::from_ptr(uts.machine.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}
