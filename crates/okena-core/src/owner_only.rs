//! Windows access control that grants the current user, and no one else, full
//! access: the counterpart of Unix modes 0700/0600 for the AI proxy's
//! directory and its management pipe.

use std::ffi::c_void;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    SE_FILE_OBJECT, SetNamedSecurityInfoW,
};
use windows_sys::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, GetTokenInformation,
    PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
    TOKEN_USER, TokenUser,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// A security descriptor whose only entry gives the current user full access,
/// with inheritance from the parent turned off.
pub struct OwnerOnly(PSECURITY_DESCRIPTOR);

impl OwnerOnly {
    /// `inherit` lets files and directories created inside take the entry over.
    pub fn new(inherit: bool) -> io::Result<Self> {
        let flags = if inherit { "OICI" } else { "" };
        let sddl = format!("D:P(A;{flags};FA;;;{})", current_user_sid()?);
        let sddl = wide(std::ffi::OsStr::new(&sddl));
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: `sddl` is NUL-terminated; the descriptor is freed in `Drop`.
        let converted = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        };
        if converted == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(descriptor))
    }

    /// Replace the access list of `path` with this one. Entries that children
    /// inherited are recomputed, so existing files inside follow too.
    pub fn apply(&self, path: &Path) -> io::Result<()> {
        let mut present = 0;
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let mut defaulted = 0;
        // SAFETY: `self.0` is a valid descriptor; `dacl` points into it.
        if unsafe { GetSecurityDescriptorDacl(self.0, &mut present, &mut dacl, &mut defaulted) }
            == 0
        {
            return Err(io::Error::last_os_error());
        }
        let path = wide(path.as_os_str());
        // SAFETY: `path` is NUL-terminated and `dacl` lives as long as `self`.
        let status = unsafe {
            SetNamedSecurityInfoW(
                path.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                dacl,
                std::ptr::null(),
            )
        };
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        Ok(())
    }

    /// Call `create` with a `SECURITY_ATTRIBUTES` pointer that carries this
    /// descriptor, e.g. for a named pipe.
    pub fn with_attributes<T>(&self, create: impl FnOnce(*mut c_void) -> T) -> T {
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0,
            bInheritHandle: 0,
        };
        create(std::ptr::from_mut(&mut attributes).cast())
    }
}

impl Drop for OwnerOnly {
    fn drop(&mut self) {
        // SAFETY: allocated by `ConvertStringSecurityDescriptorToSecurityDescriptorW`.
        unsafe { LocalFree(self.0) };
    }
}

/// The current process user's SID in string form (`S-1-5-21-…`).
fn current_user_sid() -> io::Result<String> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: the pseudo handle of the current process needs no closing.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let sid = token_user_sid(token);
    // SAFETY: `token` was opened above.
    unsafe { CloseHandle(token) };
    sid
}

fn token_user_sid(token: HANDLE) -> io::Result<String> {
    let mut length = 0;
    // SAFETY: a null buffer only asks for the required length.
    unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut length) };
    // `u64` elements keep the buffer aligned for `TOKEN_USER`.
    let mut buffer = vec![0u64; (length as usize).div_ceil(size_of::<u64>())];
    // SAFETY: `buffer` holds at least `length` bytes.
    if unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            length,
            &mut length,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `GetTokenInformation(TokenUser)` filled `buffer` with a `TOKEN_USER`.
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    let mut string = std::ptr::null_mut();
    // SAFETY: the SID points into `buffer`; `string` is freed below.
    if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut string) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `string` is a NUL-terminated UTF-16 string from the call above.
    let sid = unsafe {
        let length = (0..).take_while(|&index| *string.add(index) != 0).count();
        String::from_utf16_lossy(std::slice::from_raw_parts(string, length))
    };
    // SAFETY: allocated by `ConvertSidToStringSidW`.
    unsafe { LocalFree(string.cast()) };
    Ok(sid)
}

fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}
