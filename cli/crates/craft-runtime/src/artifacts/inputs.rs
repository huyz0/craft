use super::*;
use craft_platform::InputRoot;

fn input_error(error: io::Error) -> StoreError {
    match error.kind() {
        io::ErrorKind::InvalidInput | io::ErrorKind::PermissionDenied => StoreError::Invalid,
        _ => StoreError::Unavailable,
    }
}

impl NativeArtifacts {
    /// Snapshots a host-selected relative workspace file into immutable artifact storage.
    /// Size, source-change and optional digest checks finish before publication. No source
    /// path is persisted or reopened by the resulting receipt; providers receive the snapshot.
    pub fn import_file(
        &self,
        input_root: &InputRoot,
        path: &Path,
        spec: ArtifactSpec,
        expected_digest: Option<&str>,
    ) -> Result<ArtifactReceipt, StoreError> {
        check(&self.control)?;
        spec.validate()?;
        if let Some(expected) = expected_digest
            && !expected.strip_prefix("sha256:").is_some_and(|hash| {
                hash.len() == 64
                    && hash
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
        {
            return Err(StoreError::Invalid);
        }
        let maximum = spec.max_bytes;
        if maximum > self.max_bytes {
            return Err(StoreError::TooLarge);
        }
        let mut input = input_root.read_file(path).map_err(input_error)?;
        if input.size() > maximum {
            return Err(StoreError::TooLarge);
        }
        input.verify_unchanged().map_err(|_| StoreError::Conflict)?;
        let mut writer = self.begin(spec)?;
        let mut digest = expected_digest.map(|_| Sha256::new());
        let mut total = 0_u64;
        let mut buffer = [0_u8; STREAM_BYTES];
        loop {
            check(&self.control)?;
            let count = match input.read(&mut buffer) {
                Ok(count) => count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(input_error(error)),
            };
            if count == 0 {
                break;
            }
            total = total
                .checked_add(count as u64)
                .ok_or(StoreError::TooLarge)?;
            if total > maximum {
                return Err(StoreError::TooLarge);
            }
            if total > input.size() {
                return Err(StoreError::Conflict);
            }
            writer.write_all(&buffer[..count]).map_err(|error| {
                error
                    .get_ref()
                    .and_then(|cause| cause.downcast_ref::<StoreError>())
                    .copied()
                    .unwrap_or(StoreError::Unavailable)
            })?;
            if let Some(digest) = &mut digest {
                digest.update(&buffer[..count]);
            }
        }
        input.verify_unchanged().map_err(|_| StoreError::Conflict)?;
        if total != input.size() {
            return Err(StoreError::Conflict);
        }
        if let (Some(digest), Some(expected)) = (digest, expected_digest)
            && finish_digest(digest) != expected
        {
            return Err(StoreError::Conflict);
        }
        check(&self.control)?;
        writer.finish()
    }
}
