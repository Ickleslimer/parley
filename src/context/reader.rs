//! Incremental shared JSONL reader with byte offsets and fail-closed records.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use super::error::{ContextError, ErrorKind};
use super::winfile::{open_shared_read, source_identity, SourceIdentity};

pub(crate) const MAX_PHYSICAL_RECORD: usize = 8 * 1024 * 1024;
const BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];
const READ_CHUNK: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PhysicalRecord {
    pub start_offset: u64,
    pub end_offset: u64,
    pub text: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ReadBatch {
    pub records: Vec<PhysicalRecord>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub incomplete_tail: Vec<u8>,
    pub complete_end: u64,
}

pub(crate) struct SharedJsonlReader {
    file: File,
    identity: SourceIdentity,
    max_physical_record: usize,
}

impl SharedJsonlReader {
    pub(crate) fn open(path: &Path) -> Result<Self, ContextError> {
        Self::open_with_limit(path, MAX_PHYSICAL_RECORD)
    }

    pub(crate) fn open_with_limit(
        path: &Path,
        max_physical_record: usize,
    ) -> Result<Self, ContextError> {
        let file = open_shared_read(path)?;
        let identity = source_identity(&file, path)?;
        Ok(Self {
            file,
            identity,
            max_physical_record,
        })
    }

    pub(crate) fn identity(&self) -> &SourceIdentity {
        &self.identity
    }

    pub(crate) fn file_len(&self) -> Result<u64, ContextError> {
        Ok(self.file.metadata()?.len())
    }

    pub(crate) fn read_from(&mut self, start: u64) -> Result<ReadBatch, ContextError> {
        self.read_range(start, None)
    }

    pub(crate) fn read_until(&mut self, start: u64, end: u64) -> Result<ReadBatch, ContextError> {
        if end < start {
            return Err(ContextError::mismatch(format!(
                "source range end {end} precedes start {start}"
            )));
        }
        self.read_range(start, Some(end))
    }

    fn read_range(
        &mut self,
        start: u64,
        exact_end: Option<u64>,
    ) -> Result<ReadBatch, ContextError> {
        let size = self.file_len()?;
        if start > size {
            return Err(ContextError::new(
                ErrorKind::Truncated,
                format!("source truncated below offset {start} (size {size})"),
            ));
        }
        if let Some(end) = exact_end {
            if end > size {
                return Err(ContextError::new(
                    ErrorKind::Truncated,
                    format!("source truncated below offset {end} (size {size})"),
                ));
            }
        }

        let mut pos = self.seek_start(start)?;
        let complete_begin = pos;
        let stop_at = exact_end.unwrap_or(size);
        let mut pending = Vec::new();
        let mut pending_start = pos;
        let mut records = Vec::new();
        let mut buf = [0_u8; READ_CHUNK];

        while pos < stop_at {
            let want = ((stop_at - pos) as usize).min(buf.len());
            let read = self.file.read(&mut buf[..want])?;
            if read == 0 {
                break;
            }
            let chunk = &buf[..read];
            let mut consumed = 0usize;
            while consumed < chunk.len() {
                if pending.len() > self.max_physical_record {
                    return Err(ContextError::malformed(format!(
                        "physical JSONL record exceeds {} bytes",
                        self.max_physical_record
                    )));
                }
                match chunk[consumed..].iter().position(|&b| b == b'\n') {
                    Some(rel) => {
                        let line_chunk = &chunk[consumed..consumed + rel];
                        if pending.len().saturating_add(line_chunk.len()) > self.max_physical_record
                        {
                            return Err(ContextError::malformed(format!(
                                "physical JSONL record exceeds {} bytes",
                                self.max_physical_record
                            )));
                        }
                        pending.extend_from_slice(line_chunk);
                        let end_offset = pos + (consumed + rel + 1) as u64;
                        if !pending.is_empty() {
                            if pending.last() == Some(&b'\r') {
                                pending.pop();
                            }
                            if !pending.is_empty() {
                                records.push(complete_record(pending_start, end_offset, &pending)?);
                            }
                        }
                        pending.clear();
                        consumed += rel + 1;
                        pending_start = end_offset;
                    }
                    None => {
                        let rest = &chunk[consumed..];
                        if pending.len().saturating_add(rest.len()) > self.max_physical_record {
                            return Err(ContextError::malformed(format!(
                                "physical JSONL record exceeds {} bytes",
                                self.max_physical_record
                            )));
                        }
                        pending.extend_from_slice(rest);
                        consumed = chunk.len();
                    }
                }
            }
            pos += read as u64;
        }

        if let Some(end) = exact_end {
            if pos != end {
                return Err(ContextError::mismatch(format!(
                    "source range ended at {pos}, expected {end}"
                )));
            }
            if !pending.is_empty() {
                return Err(ContextError::mismatch(
                    "source range is not aligned to a complete JSONL record",
                ));
            }
            return Ok(ReadBatch {
                records,
                incomplete_tail: Vec::new(),
                complete_end: end.max(complete_begin),
            });
        }

        Ok(ReadBatch {
            complete_end: pending_start,
            records,
            incomplete_tail: pending,
        })
    }

    fn seek_start(&mut self, start: u64) -> Result<u64, ContextError> {
        if start == 0 {
            self.file.seek(SeekFrom::Start(0))?;
            let mut bom = [0_u8; 3];
            let read = self.file.read(&mut bom)?;
            if read == 3 && bom == BOM {
                return Ok(3);
            }
            self.file.seek(SeekFrom::Start(0))?;
            return Ok(0);
        }
        self.file.seek(SeekFrom::Start(start))?;
        Ok(start)
    }
}

pub(crate) fn read_first_complete_record(
    path: &Path,
) -> Result<Option<PhysicalRecord>, ContextError> {
    let mut file = open_shared_read(path)?;
    let mut start_offset = 0_u64;
    let mut prefix = [0_u8; 3];
    let prefix_len = file.read(&mut prefix)?;
    if prefix_len == 3 && prefix == BOM {
        start_offset = 3;
    } else {
        file.seek(SeekFrom::Start(0))?;
    }

    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let read = file.read(&mut chunk)?;
        if read == 0 {
            return Ok(None);
        }
        if let Some(newline) = chunk[..read].iter().position(|byte| *byte == b'\n') {
            if bytes.len().saturating_add(newline) > MAX_PHYSICAL_RECORD {
                return Err(ContextError::malformed(format!(
                    "physical JSONL record exceeds {} bytes",
                    MAX_PHYSICAL_RECORD
                )));
            }
            let end_offset = start_offset + bytes.len() as u64 + newline as u64 + 1;
            bytes.extend_from_slice(&chunk[..newline]);
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
            if bytes.is_empty() {
                return Ok(None);
            }
            return complete_record(start_offset, end_offset, &bytes).map(Some);
        }
        if bytes.len().saturating_add(read) > MAX_PHYSICAL_RECORD {
            return Err(ContextError::malformed(format!(
                "physical JSONL record exceeds {} bytes",
                MAX_PHYSICAL_RECORD
            )));
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
}

fn complete_record(start: u64, end: u64, bytes: &[u8]) -> Result<PhysicalRecord, ContextError> {
    let text = std::str::from_utf8(bytes).map_err(|_| {
        ContextError::malformed(format!("invalid UTF-8 in JSONL record at offset {start}"))
    })?;
    Ok(PhysicalRecord {
        start_offset: start,
        end_offset: end,
        text: text.to_string(),
    })
}
