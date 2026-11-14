//! A verified opened snapshot never reacquires filesystem authority from receipt paths.
use super::*;
use std::io::{Seek, SeekFrom};
pub struct ArtifactInput {
    store: NativeArtifacts,
    file: File,
    receipt: ArtifactReceipt,
    digest: Option<Sha256>,
    bytes: u64,
    complete: bool,
    failed: bool,
}
impl ArtifactInput {
    pub(crate) fn is_fresh(&self) -> bool {
        self.bytes == 0 && !self.complete && !self.failed && self.digest.is_some()
    }

    pub fn receipt(&self) -> &ArtifactReceipt {
        &self.receipt
    }
}
impl NativeArtifacts {
    pub fn open_verified_stream(
        &self,
        name: &str,
        expected_digest: &str,
    ) -> Result<ArtifactInput, StoreError> {
        check(&self.control)?;
        if !expected_digest
            .strip_prefix("sha256:")
            .is_some_and(|digest| {
                digest.len() == 64
                    && digest
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
        {
            return Err(StoreError::Invalid);
        }
        let (receipt, mut file) = self.open_artifact(name)?.ok_or(StoreError::Invalid)?;
        if receipt.digest != expected_digest {
            return Err(StoreError::Conflict);
        }
        self.verify(&receipt, &mut file, None)?;
        file.seek(SeekFrom::Start(0)).map_err(io_error)?;
        check(&self.control)?;
        Ok(ArtifactInput {
            store: self.clone(),
            file,
            receipt,
            digest: Some(Sha256::new()),
            bytes: 0,
            complete: false,
            failed: false,
        })
    }
}
impl Read for ArtifactInput {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let result = (|| {
            check(&self.store.control)?;
            if self.failed {
                return Err(StoreError::Corrupt);
            }
            if self.complete {
                return Ok(0);
            }
            let remaining = self
                .receipt
                .bytes
                .checked_sub(self.bytes)
                .ok_or(StoreError::Corrupt)?;
            if remaining == 0 {
                let mut extra = [0];
                if self.file.read(&mut extra).map_err(io_error)? != 0 {
                    return Err(StoreError::Corrupt);
                }
                let digest = self.digest.take().ok_or(StoreError::Corrupt)?;
                if finish_digest(digest) != self.receipt.digest {
                    return Err(StoreError::Corrupt);
                }
                check(&self.store.control)?;
                self.complete = true;
                return Ok(0);
            }
            let maximum = buffer
                .len()
                .min(STREAM_BYTES)
                .min(usize::try_from(remaining).unwrap_or(STREAM_BYTES));
            let count = self.file.read(&mut buffer[..maximum]).map_err(io_error)?;
            if count == 0 {
                return Err(StoreError::Corrupt);
            }
            self.digest
                .as_mut()
                .ok_or(StoreError::Corrupt)?
                .update(&buffer[..count]);
            self.bytes += count as u64;
            check(&self.store.control)?;
            Ok(count)
        })();
        if result.is_err() {
            self.failed = true;
        }
        result.map_err(io::Error::other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::SystemClock;
    use craft_core::control::CancellationToken;
    #[test]
    fn second_pass_rejects_changed_truncated_and_extended_source_bytes()
    -> Result<(), Box<dyn std::error::Error>> {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        fs::create_dir_all(&parent)?;
        let parent = parent.canonicalize()?;
        let root = parent.join(crate::profiles::random_id("stream-corruption-")?);
        let control = InvocationControl::new(
            Arc::new(SystemClock::default()),
            CancellationToken::default(),
            10_000,
        )?;
        let store = NativeArtifacts::open(&root, 100, &control)?;
        for (name, changed) in [
            ("changed", b"altered!".as_slice()),
            ("short", b"short".as_slice()),
            ("extended", b"original-extra".as_slice()),
        ] {
            let mut writer = store.begin(ArtifactSpec {
                name: name.into(),
                media_type: "application/octet-stream".into(),
                max_bytes: 100,
                source: None,
            })?;
            writer.write_all(b"original")?;
            let receipt = writer.finish()?;
            let mut input = store.open_verified_stream(name, &receipt.digest)?;
            let path = receipt.local_path.as_ref().ok_or("path")?;
            // A write-sharing read handle models platforms where another writer can change
            // an opened inode. Production Windows read handles additionally deny that writer.
            input.file = File::open(path)?;
            fs::write(path, changed)?;
            let mut observed = Vec::new();
            let error = input
                .read_to_end(&mut observed)
                .err()
                .ok_or("expected changed source")?;
            assert_eq!(
                error
                    .get_ref()
                    .and_then(|v| v.downcast_ref::<StoreError>())
                    .copied(),
                Some(StoreError::Corrupt)
            );
            assert!(!input.complete);
            assert!(input.failed);
        }
        drop(store);
        let checked = root.canonicalize()?;
        assert!(checked.starts_with(&parent) && checked != parent);
        fs::remove_dir_all(checked)?;
        Ok(())
    }
}
