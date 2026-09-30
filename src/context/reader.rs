//! Incremental shared JSONL reader with byte offsets and fail-closed records.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use super::error::{ContextError, ErrorKind};
use super::excluded_bytes::is_skippable_invalid_utf8_record;
use super::oversized::OversizedJsonScanner;
use super::winfile::{open_shared_read, source_identity, SourceIdentity};

pub(crate) const MAX_PHYSICAL_RECORD: usize = 8 * 1024 * 1024;
const BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];
const READ_CHUNK: usize = 64 * 1024;
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PhysicalRecord {
    pub start_offset: u64,
    pub end_offset: u64,
    pub text: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ScanSummary {
    pub complete_end: u64,
    pub first_record_start: Option<u64>,
    pub record_count: u64,
    pub fingerprint: String,
    #[cfg_attr(not(test), allow(dead_code))]
    pub skipped_oversized: u64,
    #[cfg_attr(not(test), allow(dead_code))]
    pub skipped_invalid_utf8: u64,
    #[cfg_attr(not(test), allow(dead_code))]
    pub incomplete_tail_len: u64,
    #[cfg_attr(not(test), allow(dead_code))]
    pub incomplete_tail_oversized: bool,
}

#[derive(Clone, Debug)]
#[cfg(test)]
pub(crate) struct ReadBatch {
    pub records: Vec<PhysicalRecord>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub incomplete_tail: Vec<u8>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub incomplete_tail_len: u64,
    #[cfg_attr(not(test), allow(dead_code))]
    pub incomplete_tail_oversized: bool,
    pub complete_end: u64,
    pub record_count: u64,
    pub fingerprint: String,
    #[cfg_attr(not(test), allow(dead_code))]
    pub skipped_oversized: u64,
    #[cfg_attr(not(test), allow(dead_code))]
    pub skipped_invalid_utf8: u64,
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

    #[cfg(test)]
    pub(crate) fn read_from(&mut self, start: u64) -> Result<ReadBatch, ContextError> {
        let mut records = Vec::new();
        let (summary, incomplete_tail) = self.scan_range(start, None, |record| {
            records.push(record);
            Ok(())
        })?;
        Ok(ReadBatch {
            records,
            incomplete_tail,
            incomplete_tail_len: summary.incomplete_tail_len,
            incomplete_tail_oversized: summary.incomplete_tail_oversized,
            complete_end: summary.complete_end,
            record_count: summary.record_count,
            fingerprint: summary.fingerprint,
            skipped_oversized: summary.skipped_oversized,
            skipped_invalid_utf8: summary.skipped_invalid_utf8,
        })
    }

    #[cfg(test)]
    pub(crate) fn read_until(&mut self, start: u64, end: u64) -> Result<ReadBatch, ContextError> {
        let mut records = Vec::new();
        let (summary, incomplete_tail) = self.scan_range(start, Some(end), |record| {
            records.push(record);
            Ok(())
        })?;
        Ok(ReadBatch {
            records,
            incomplete_tail,
            incomplete_tail_len: summary.incomplete_tail_len,
            incomplete_tail_oversized: summary.incomplete_tail_oversized,
            complete_end: summary.complete_end,
            record_count: summary.record_count,
            fingerprint: summary.fingerprint,
            skipped_oversized: summary.skipped_oversized,
            skipped_invalid_utf8: summary.skipped_invalid_utf8,
        })
    }

    pub(crate) fn scan_from<F>(
        &mut self,
        start: u64,
        on_record: F,
    ) -> Result<ScanSummary, ContextError>
    where
        F: FnMut(PhysicalRecord) -> Result<(), ContextError>,
    {
        self.scan_range(start, None, on_record)
            .map(|(summary, _)| summary)
    }

    pub(crate) fn scan_until<F>(
        &mut self,
        start: u64,
        end: u64,
        on_record: F,
    ) -> Result<ScanSummary, ContextError>
    where
        F: FnMut(PhysicalRecord) -> Result<(), ContextError>,
    {
        if end < start {
            return Err(ContextError::mismatch(format!(
                "source range end {end} precedes start {start}"
            )));
        }
        self.scan_range(start, Some(end), on_record)
            .map(|(summary, _)| summary)
    }

    fn scan_range<F>(
        &mut self,
        start: u64,
        exact_end: Option<u64>,
        mut on_record: F,
    ) -> Result<(ScanSummary, Vec<u8>), ContextError>
    where
        F: FnMut(PhysicalRecord) -> Result<(), ContextError>,
    {
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
        let mut pending = PendingRecord::new(pos);
        let mut pending_start = pos;
        let mut fingerprint = RangeFingerprint::new();
        let mut first_record_start = None;
        let mut skipped_oversized = 0_u64;
        let mut skipped_invalid_utf8 = 0_u64;
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
                match chunk[consumed..].iter().position(|&byte| byte == b'\n') {
                    Some(relative) => {
                        let bytes = &chunk[consumed..consumed + relative];
                        pending.feed(bytes, self.max_physical_record);
                        fingerprint.feed(bytes);
                        let end_offset = pos + (consumed + relative + 1) as u64;
                        let fingerprint_record = fingerprint.finish_record();
                        match pending.finish(end_offset)? {
                            CompletedRecord::Empty => {
                                debug_assert!(!fingerprint_record);
                            }
                            CompletedRecord::Retained(record) => {
                                debug_assert!(fingerprint_record);
                                first_record_start.get_or_insert(record.start_offset);
                                on_record(record)?;
                            }
                            CompletedRecord::SkippedOversized => {
                                debug_assert!(fingerprint_record);
                                first_record_start.get_or_insert(pending_start);
                                skipped_oversized = skipped_oversized.saturating_add(1);
                            }
                            CompletedRecord::SkippedInvalidUtf8 => {
                                debug_assert!(fingerprint_record);
                                first_record_start.get_or_insert(pending_start);
                                skipped_invalid_utf8 = skipped_invalid_utf8.saturating_add(1);
                            }
                        }
                        consumed += relative + 1;
                        pending_start = end_offset;
                        pending = PendingRecord::new(pending_start);
                    }
                    None => {
                        let bytes = &chunk[consumed..];
                        pending.feed(bytes, self.max_physical_record);
                        fingerprint.feed(bytes);
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
            if pending.byte_len() != 0 {
                return Err(ContextError::mismatch(
                    "source range is not aligned to a complete JSONL record",
                ));
            }
        }

        let incomplete_tail_len = pending.byte_len();
        let incomplete_tail_oversized = pending.is_oversized();
        let incomplete_tail = pending.into_buffered_tail();
        let complete_end = match exact_end {
            Some(end) => end.max(complete_begin),
            None => pending_start,
        };
        Ok((
            ScanSummary {
                complete_end,
                first_record_start,
                record_count: fingerprint.record_count,
                fingerprint: format!("{:016x}", fingerprint.committed_hash),
                skipped_oversized,
                skipped_invalid_utf8,
                incomplete_tail_len,
                incomplete_tail_oversized,
            },
            incomplete_tail,
        ))
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

enum PendingStorage {
    Buffered(Vec<u8>),
    Oversized {
        scanner: Option<OversizedJsonScanner>,
        error: Option<ContextError>,
    },
}

struct PendingRecord {
    start_offset: u64,
    byte_len: u64,
    storage: PendingStorage,
}

impl PendingRecord {
    fn new(start_offset: u64) -> Self {
        Self {
            start_offset,
            byte_len: 0,
            storage: PendingStorage::Buffered(Vec::new()),
        }
    }

    fn feed(&mut self, bytes: &[u8], max_physical_record: usize) {
        self.byte_len = self.byte_len.saturating_add(bytes.len() as u64);
        match &mut self.storage {
            PendingStorage::Buffered(buffer)
                if buffer.len().saturating_add(bytes.len()) <= max_physical_record =>
            {
                buffer.extend_from_slice(bytes);
            }
            PendingStorage::Buffered(buffer) => {
                let retained = std::mem::take(buffer);
                let mut scanner = OversizedJsonScanner::new();
                let error = scanner
                    .feed(&retained)
                    .and_then(|_| scanner.feed(bytes))
                    .err();
                self.storage = PendingStorage::Oversized {
                    scanner: if error.is_none() { Some(scanner) } else { None },
                    error,
                };
            }
            PendingStorage::Oversized { scanner, error } => {
                if error.is_none() {
                    if let Some(active) = scanner {
                        if let Err(failure) = active.feed(bytes) {
                            *error = Some(failure);
                            *scanner = None;
                        }
                    }
                }
            }
        }
    }

    fn finish(self, end_offset: u64) -> Result<CompletedRecord, ContextError> {
        match self.storage {
            PendingStorage::Buffered(mut bytes) => {
                if bytes.last() == Some(&b'\r') {
                    bytes.pop();
                }
                if bytes.is_empty() {
                    return Ok(CompletedRecord::Empty);
                }
                if std::str::from_utf8(&bytes).is_err() {
                    if is_skippable_invalid_utf8_record(&bytes) {
                        return Ok(CompletedRecord::SkippedInvalidUtf8);
                    }
                    return Err(ContextError::malformed(format!(
                        "invalid UTF-8 in JSONL record at offset {}",
                        self.start_offset
                    )));
                }
                complete_record(self.start_offset, end_offset, &bytes)
                    .map(CompletedRecord::Retained)
            }
            PendingStorage::Oversized { scanner, error } => {
                let result = match error {
                    Some(error) => Err(error),
                    None => scanner
                        .ok_or_else(|| {
                            ContextError::malformed("oversized JSON scanner is unavailable")
                        })?
                        .finish(),
                };
                result.map_err(|error| {
                    ContextError::malformed(format!(
                        "oversized JSONL record at offset {}: {}",
                        self.start_offset, error.message
                    ))
                })?;
                Ok(CompletedRecord::SkippedOversized)
            }
        }
    }

    fn byte_len(&self) -> u64 {
        self.byte_len
    }

    fn is_oversized(&self) -> bool {
        matches!(self.storage, PendingStorage::Oversized { .. })
    }

    fn into_buffered_tail(self) -> Vec<u8> {
        match self.storage {
            PendingStorage::Buffered(bytes) => bytes,
            PendingStorage::Oversized { .. } => Vec::new(),
        }
    }
}

enum CompletedRecord {
    Empty,
    Retained(PhysicalRecord),
    SkippedOversized,
    SkippedInvalidUtf8,
}

struct RangeFingerprint {
    committed_hash: u64,
    record_count: u64,
    line_hash: u64,
    line_started: bool,
    delayed: Option<u8>,
}

impl RangeFingerprint {
    fn new() -> Self {
        Self {
            committed_hash: FNV_OFFSET,
            record_count: 0,
            line_hash: FNV_OFFSET,
            line_started: false,
            delayed: None,
        }
    }

    fn feed(&mut self, bytes: &[u8]) {
        for byte in bytes {
            if let Some(previous) = self.delayed.replace(*byte) {
                self.commit_line_byte(previous);
            }
        }
    }

    fn finish_record(&mut self) -> bool {
        if let Some(last) = self.delayed.take() {
            if last != b'\r' {
                self.commit_line_byte(last);
            }
        }
        let committed = self.line_started;
        if committed {
            self.committed_hash = self.line_hash;
            self.record_count = self.record_count.saturating_add(1);
        }
        self.line_hash = self.committed_hash;
        self.line_started = false;
        committed
    }

    fn commit_line_byte(&mut self, byte: u8) {
        if !self.line_started {
            if self.record_count > 0 {
                self.line_hash = fnv_byte(self.line_hash, b'\n');
            }
            self.line_started = true;
        }
        self.line_hash = fnv_byte(self.line_hash, byte);
    }
}

fn fnv_byte(hash: u64, byte: u8) -> u64 {
    (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME)
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
