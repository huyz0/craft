use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use clap::Args;
use craft_factory_evidence::{
    queue_identity::{IdentityConfig, Role, identifier},
    sha256,
};

use crate::{Result, reject};

#[derive(Args)]
pub struct Options {
    id: String,
    #[arg(long, value_enum)]
    role: Role,
    /// New protected token file in an existing directory.
    #[arg(long)]
    token_file: PathBuf,
    /// Limit a verifier to an exact repository. Repeat for multiple repositories.
    #[arg(long = "verifier-repository")]
    verifier_repositories: Vec<String>,
    /// Limit a context reader to exact repositories. Repeat for multiple repositories.
    #[arg(long = "context-repository")]
    context_repositories: Vec<String>,
}

pub fn issue(options: Options) -> Result<IdentityConfig> {
    if !identifier(&options.id) {
        return reject("invalid queue identity ID");
    }
    let mut identity = IdentityConfig {
        id: options.id,
        role: options.role,
        token_sha256: String::new(),
        verifier_repositories: (!options.verifier_repositories.is_empty())
            .then_some(options.verifier_repositories),
        context_repositories: (!options.context_repositories.is_empty())
            .then_some(options.context_repositories),
    };
    identity.repository_scope()?;
    let name = options
        .token_file
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.contains([':', '\\']) && !name.chars().any(char::is_control))
        .ok_or("invalid queue token file name")?;
    let directory = options
        .token_file
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .canonicalize()?;
    let mut nonce = [0u8; 32];
    getrandom::getrandom(&mut nonce).map_err(|_| "could not generate a queue token")?;
    let token = URL_SAFE_NO_PAD.encode(nonce);
    let mut file = create_private_file(&directory.join(name))?;
    writeln!(file, "{token}")?;
    file.sync_all()?;
    identity.token_sha256 = sha256(token.as_bytes())[7..].to_owned();
    Ok(identity)
}

#[cfg(unix)]
fn create_private_file(path: &Path) -> Result<File> {
    use std::{
        fs::OpenOptions,
        os::unix::fs::{OpenOptionsExt, PermissionsExt},
    };

    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    if file.metadata()?.permissions().mode() & 0o077 != 0 {
        return reject("queue token destination must enforce owner-only file permissions");
    }
    Ok(file)
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn create_private_file(path: &Path) -> Result<File> {
    use std::{
        io,
        mem::size_of,
        os::windows::{ffi::OsStrExt, io::FromRawHandle},
        ptr::null_mut,
    };
    use windows_sys::Win32::{
        Foundation::{GENERIC_WRITE, INVALID_HANDLE_VALUE, LocalFree},
        Security::{
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
            },
            PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
        },
        Storage::FileSystem::{
            CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL, GetVolumeInformationByHandleW,
        },
        System::SystemServices::FILE_PERSISTENT_ACLS,
    };

    // The protected DACL grants access only to the file owner and prevents inherited ACEs.
    let sddl: Vec<u16> = "D:P(A;;FA;;;OW)".encode_utf16().chain([0]).collect();
    let name: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    // Both strings are nul-terminated and the descriptor stays alive through CreateFileW.
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            null_mut(),
        ) == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let handle = CreateFileW(
            name.as_ptr(),
            GENERIC_WRITE,
            0,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        );
        let opened = if handle == INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            Ok(File::from_raw_handle(handle))
        };
        LocalFree(descriptor);
        let file = opened?;
        let mut flags = 0;
        if GetVolumeInformationByHandleW(
            handle,
            null_mut(),
            0,
            null_mut(),
            null_mut(),
            &mut flags,
            null_mut(),
            0,
        ) == 0
            || flags & FILE_PERSISTENT_ACLS == 0
        {
            return reject("queue token destination must support persistent Windows file ACLs");
        }
        Ok(file)
    }
}
