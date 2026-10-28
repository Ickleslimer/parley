use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
#[cfg(test)]
use std::path::Path;

use super::error::{ContextError, ErrorKind};
use super::lineage::SourcePart;
use super::reader::read_first_complete_record;
use super::root::ResolvedSource;
use super::winfile::{open_shared_read, source_identity};

struct OpenPart {
    file: File,
    source: SourcePart,
}

pub(super) struct LogicalFile {
    parts: Vec<OpenPart>,
    position: u64,
    selection: Option<(std::path::PathBuf, std::path::PathBuf, String)>,
}

impl LogicalFile {
    pub(super) fn open(source: &ResolvedSource) -> Result<Self, ContextError> {
        let parts = if source.parts.is_empty() {
            vec![SourcePart {
                path: source.path.clone(),
                identity: source.identity.clone(),
                extent: super::winfile::HistoryIdentity {
                    volume_serial: source.identity.volume_serial,
                    file_index: source.identity.file_index,
                    logical_start: 0,
                    physical_start: 0,
                    physical_end: None,
                    header_fingerprint: String::new(),
                },
            }]
        } else {
            source.parts.clone()
        };
        let mut opened = Vec::new();
        for part in parts {
            let mut file = open_shared_read(&part.path)?;
            let live = source_identity(&file, &part.path)?;
            if !part.identity.same_file(&live)
                || live.canonical_path != part.identity.canonical_path
            {
                return Err(ContextError::new(
                    ErrorKind::Replaced,
                    "history segment replaced while opening",
                ));
            }
            if !part.extent.header_fingerprint.is_empty() {
                let header = super::reader::read_first_from_file(&mut file)?
                    .ok_or_else(|| ContextError::malformed("history header disappeared"))?;
                if format!("{:016x}", crate::signals::fnv1a_64(&header.text))
                    != part.extent.header_fingerprint
                {
                    return Err(ContextError::mismatch(
                        "history header changed while opening",
                    ));
                }
            }
            opened.push(OpenPart { file, source: part });
        }
        let stream = Self {
            parts: opened,
            position: 0,
            selection: source
                .selection
                .as_ref()
                .map(|(home, path)| (home.clone(), path.clone(), source.session_id.clone())),
        };
        stream.len()?;
        Ok(stream)
    }

    #[cfg(test)]
    pub(super) fn single(
        path: &Path,
    ) -> Result<(Self, super::winfile::SourceIdentity), ContextError> {
        let file = open_shared_read(path)?;
        let identity = source_identity(&file, path)?;
        let source = ResolvedSource {
            path: path.to_path_buf(),
            session_id: String::new(),
            identity: identity.clone(),
            parts: Vec::new(),
            selection: None,
        };
        Ok((Self::open(&source)?, identity))
    }

    pub(super) fn len(&self) -> Result<u64, ContextError> {
        let mut total = 0;
        for part in &self.parts {
            let extent = &part.source.extent;
            let size = part.file.metadata()?.len();
            let end = extent.physical_end.unwrap_or(size);
            if end > size || end < extent.physical_start {
                return Err(ContextError::new(
                    ErrorKind::Truncated,
                    "history segment truncated",
                ));
            }
            total = extent
                .logical_start
                .checked_add(end - extent.physical_start)
                .ok_or_else(|| ContextError::overflow("logical history size overflow"))?;
        }
        Ok(total)
    }

    pub(super) fn verify(&self) -> Result<(), ContextError> {
        self.len()?;
        if let Some((home, expected, session)) = &self.selection {
            let active = super::active_index::active_rollout(home, session)?.ok_or_else(|| {
                ContextError::mismatch("active source index disappeared during read")
            })?;
            if std::fs::canonicalize(active)? != *expected {
                return Err(ContextError::mismatch(
                    "active source changed during history read",
                ));
            }
        }
        for part in &self.parts {
            if part.source.extent.header_fingerprint.is_empty() {
                continue;
            }
            let current = open_shared_read(&part.source.path)?;
            let live = source_identity(&current, &part.source.path)?;
            if !part.source.identity.same_file(&live)
                || live.canonical_path != part.source.identity.canonical_path
            {
                return Err(ContextError::new(
                    ErrorKind::Replaced,
                    "history segment replaced during read",
                ));
            }
            if !part.source.extent.header_fingerprint.is_empty() {
                let header = read_first_complete_record(&part.source.path)?
                    .ok_or_else(|| ContextError::malformed("history header disappeared"))?;
                if format!("{:016x}", crate::signals::fnv1a_64(&header.text))
                    != part.source.extent.header_fingerprint
                {
                    return Err(ContextError::mismatch("history header changed during read"));
                }
            }
        }
        Ok(())
    }

    pub(super) fn seek(&mut self, position: u64) {
        self.position = position;
    }

    pub(super) fn read(&mut self, output: &mut [u8]) -> Result<usize, ContextError> {
        for part in &mut self.parts {
            let extent = &part.source.extent;
            let end = extent.physical_end.unwrap_or(part.file.metadata()?.len());
            let logical_end = extent.logical_start + end - extent.physical_start;
            if self.position < extent.logical_start || self.position >= logical_end {
                continue;
            }
            let physical = extent.physical_start + self.position - extent.logical_start;
            part.file.seek(SeekFrom::Start(physical))?;
            let length = output.len().min((logical_end - self.position) as usize);
            let read = part.file.read(&mut output[..length])?;
            self.position += read as u64;
            return Ok(read);
        }
        Ok(0)
    }
}
