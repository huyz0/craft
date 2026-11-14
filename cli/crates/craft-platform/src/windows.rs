use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::mem::{MaybeUninit, size_of};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::ptr::{addr_of, null_mut};

use windows_sys::Win32::Foundation::{
    ERROR_INSUFFICIENT_BUFFER, GetLastError, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, DACL_SECURITY_INFORMATION, GetSecurityDescriptorControl,
    GetTokenInformation, OWNER_SECURITY_INFORMATION, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CREATE_NEW, CreateDirectoryW, CreateFileW, FILE_ALL_ACCESS,
    FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    GetFileInformationByHandle,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

pub(super) fn stdin_is_pipe() -> io::Result<bool> {
    use windows_sys::Win32::Storage::FileSystem::{FILE_TYPE_PIPE, FILE_TYPE_UNKNOWN, GetFileType};
    let input = std::io::stdin();
    // Stdin owns this borrowed handle for the duration of GetFileType; no close or read occurs.
    let kind = unsafe { GetFileType(input.as_raw_handle()) };
    if kind == FILE_TYPE_UNKNOWN {
        return Err(io::Error::other("Cannot establish stdin source kind"));
    }
    Ok(kind == FILE_TYPE_PIPE)
}

pub(super) fn input_file_identity(file: &File) -> io::Result<(u64, u64)> {
    let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // The borrowed File keeps the handle live and the output buffer covers the full native result.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // The successful call initialized every field of this fixed native structure.
    let information = unsafe { information.assume_init() };
    if information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || information.nNumberOfLinks != 1
    {
        return Err(super::denied());
    }
    Ok((
        u64::from(information.dwVolumeSerialNumber),
        u64::from(information.nFileIndexHigh) << 32 | u64::from(information.nFileIndexLow),
    ))
}

struct LocalAllocation(*mut c_void);
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        // Every allocation held here comes from a documented LocalAlloc-family API.
        unsafe {
            LocalFree(self.0);
        }
    }
}

fn wide_path(path: &Path) -> io::Result<Vec<u16>> {
    let mut value: Vec<_> = path.as_os_str().encode_wide().collect();
    if value.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "NUL in state path",
        ));
    }
    value.push(0);
    Ok(value)
}

fn sid_string(sid: *mut c_void) -> io::Result<String> {
    let mut output = null_mut();
    // The SID is borrowed from a live token/security descriptor supplied by Windows.
    if unsafe { ConvertSidToStringSidW(sid, &mut output) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let _allocation = LocalAllocation(output.cast());
    let mut length = 0;
    // ConvertSidToStringSidW returns a valid NUL-terminated LocalAlloc string.
    unsafe {
        while *output.add(length) != 0 {
            length += 1;
        }
        String::from_utf16(std::slice::from_raw_parts(output, length))
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid native SID"))
    }
}

pub(super) fn current_principal() -> io::Result<String> {
    let mut token = null_mut();
    // The pseudo process handle is borrowed; a successful call yields one owned token handle.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if token.is_null() {
        return Err(io::Error::other("Native token handle was empty"));
    }
    // Ownership transfers exactly once from OpenProcessToken into this RAII guard.
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut needed = 0;
    // A null query buffer asks Windows for the required byte count.
    let first = unsafe {
        GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut needed)
    };
    if first != 0
        || unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER
        || !(size_of::<TOKEN_USER>() as u32..=65_536).contains(&needed)
    {
        return Err(io::Error::other("Invalid native token size"));
    }
    let words = (needed as usize).div_ceil(size_of::<usize>());
    let mut buffer = vec![0_usize; words];
    let mut written = 0;
    // usize storage supplies TOKEN_USER alignment and at least the requested capacity.
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            needed,
            &mut written,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if written < size_of::<TOKEN_USER>() as u32 || written > needed {
        return Err(io::Error::other("Invalid native token result"));
    }
    // Windows initialized TOKEN_USER and its SID inside this still-live aligned buffer.
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    sid_string(user.User.Sid)
}

fn descriptor(principal: &str) -> io::Result<LocalAllocation> {
    let sddl = format!("O:{principal}D:P(A;OICI;FA;;;{principal})(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)");
    let text: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
    let mut value = null_mut();
    // The revision is SDDL_REVISION_1; the returned descriptor is owned LocalAlloc memory.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text.as_ptr(),
            1,
            &mut value,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(LocalAllocation(value))
}

pub(super) fn create_directory(path: &Path, principal: &str) -> io::Result<()> {
    let path = wide_path(path)?;
    let descriptor = descriptor(principal)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    // Both the path and descriptor remain allocated until the synchronous creation returns.
    if unsafe { CreateDirectoryW(path.as_ptr(), &attributes) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(super) fn create_file(path: &Path, principal: &str) -> io::Result<File> {
    let path = wide_path(path)?;
    let descriptor = descriptor(principal)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    // CREATE_NEW refuses replacement. Live descriptor/path buffers cover the entire call.
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            FILE_GENERIC_READ | FILE_GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    // CreateFileW transferred ownership of this valid handle to the Rust File.
    Ok(unsafe { File::from_raw_handle(handle) })
}

pub(super) fn rename_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
    let source = wide_path(source)?;
    let destination = wide_path(destination)?;
    // Both buffers remain live. Neither REPLACE_EXISTING nor COPY_ALLOWED is selected,
    // so publication cannot replace an entry or degrade into a partial cross-volume copy.
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(super) fn create_output_file(path: &Path) -> io::Result<File> {
    use windows_sys::Win32::Storage::FileSystem::DELETE;
    let path = wide_path(path)?;
    let principal = current_principal()?;
    let descriptor = descriptor(&principal)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    // CREATE_NEW and the private descriptor bind the new file. DELETE permits later
    // rename/abort through this same owned handle; write/delete sharing stays disabled.
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            FILE_GENERIC_READ | FILE_GENERIC_WRITE | DELETE,
            FILE_SHARE_READ,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    // The successful native call transfers one live handle into this Rust File.
    Ok(unsafe { File::from_raw_handle(handle) })
}

pub(super) fn directory_identity(file: &File) -> io::Result<(u64, u64)> {
    let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // The borrowed directory handle and fixed output storage remain live through the call.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // Success initializes the complete native record; identity is data, never path authority.
    let information = unsafe { information.assume_init() };
    if information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || information.dwFileAttributes
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY
            == 0
    {
        return Err(super::denied());
    }
    Ok((
        u64::from(information.dwVolumeSerialNumber),
        u64::from(information.nFileIndexHigh) << 32 | u64::from(information.nFileIndexLow),
    ))
}

pub(super) fn publish_output_file(
    file: &File,
    destination: &Path,
    replace: bool,
) -> io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_RENAME_INFO, FileRenameInfo, SetFileInformationByHandle,
    };
    let destination = wide_path(destination)?;
    let offset = std::mem::offset_of!(FILE_RENAME_INFO, FileName);
    let buffer_size = offset
        .checked_add(
            destination
                .len()
                .checked_mul(2)
                .ok_or_else(|| io::Error::other("Output rename buffer exceeds its bound"))?,
        )
        .filter(|size| *size <= 65_536)
        .ok_or_else(|| io::Error::other("Output rename buffer exceeds its bound"))?;
    let name_bytes = u32::try_from((destination.len() - 1) * 2)
        .map_err(|_| io::Error::other("Output name exceeds its bound"))?;
    let mut storage = vec![0_usize; buffer_size.div_ceil(size_of::<usize>())];
    let information = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    // Vec<usize> supplies native pointer alignment and zeroed storage for the structure
    // plus its variable UTF-16 tail. Both buffers and the borrowed file live through the call.
    unsafe {
        (*information).Anonymous.ReplaceIfExists = replace;
        (*information).RootDirectory = null_mut();
        (*information).FileNameLength = name_bytes;
        std::ptr::copy_nonoverlapping(
            destination.as_ptr(),
            std::ptr::addr_of_mut!((*information).FileName).cast::<u16>(),
            destination.len(),
        );
    }
    // The source is identified by its retained handle, not an ambient source path.
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileRenameInfo,
            information.cast(),
            buffer_size as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(super) fn discard_output_file(file: &File) -> io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle,
    };
    let information = FILE_DISPOSITION_INFO { DeleteFile: true };
    // The disposition applies only to the retained owned staging object and takes
    // effect when its last handle closes. No pathname is reopened or removed.
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            std::ptr::addr_of!(information).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(super) fn verify(file: &File, principal: &str, root: bool) -> io::Result<()> {
    let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // The file handle is borrowed and Windows initializes the full output struct on success.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let information = unsafe { information.assume_init() };
    if information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || (!root && information.nNumberOfLinks != 1)
    {
        return Err(super::denied());
    }
    let mut owner = null_mut();
    let mut dacl = null_mut();
    let mut descriptor = null_mut();
    // Owner and ACL pointers borrow the returned descriptor allocation, kept alive below.
    let status = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let _allocation = LocalAllocation(descriptor);
    if owner.is_null() || dacl.is_null() || descriptor.is_null() || sid_string(owner)? != principal
    {
        return Err(super::denied());
    }
    let mut control = 0;
    let mut revision = 0;
    // The descriptor came from GetSecurityInfo and stays allocated for this inspection.
    if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if root && control & 0x1000 == 0 {
        return Err(super::denied());
    }
    let mut owner_full_access = false;
    // The native ACL header bounds GetAce; every returned ACE is borrowed from this ACL.
    for index in 0..unsafe { (*dacl).AceCount } {
        let mut entry = null_mut();
        if unsafe { windows_sys::Win32::Security::GetAce(dacl, index as u32, &mut entry) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if entry.is_null() {
            return Err(super::denied());
        }
        let header = unsafe { &*entry.cast::<ACE_HEADER>() };
        if header.AceType != 0 {
            return Err(super::denied());
        }
        let entry = unsafe { &*entry.cast::<ACCESS_ALLOWED_ACE>() };
        let trustee = sid_string(addr_of!(entry.SidStart).cast_mut().cast())?;
        if trustee != principal && trustee != "S-1-5-18" && trustee != "S-1-5-32-544" {
            return Err(super::denied());
        }
        if trustee == principal
            && entry.Mask & FILE_ALL_ACCESS == FILE_ALL_ACCESS
            && entry.Header.AceFlags & 0x08 == 0
        {
            owner_full_access = true;
        }
    }
    if !owner_full_access {
        return Err(super::denied());
    }
    Ok(())
}
