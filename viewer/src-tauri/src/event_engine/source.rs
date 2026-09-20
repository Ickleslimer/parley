use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;

use windows_sys::Win32::Storage::FileSystem::{
    GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE,
};

use super::types::Diagnostics;

pub(crate) const MAX_PHYSICAL_LINE: usize = 8 * 1024 * 1024;
const BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];
const HEAD_CAPTURE: usize = 64;
const READ_CHUNK: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    volume_serial: u32,
    file_index: u64,
}

impl FileIdentity {
    pub(crate) fn from_file(file: &File) -> io::Result<Self> {
        let mut info = unsafe { std::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>() };
        let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            volume_serial: info.dwVolumeSerialNumber,
            file_index: ((info.nFileIndexHigh as u64) << 32) | u64::from(info.nFileIndexLow),
        })
    }
}

#[derive(Debug)]
pub(crate) struct FileCursor {
    pub identity: Option<FileIdentity>,
    pub offset: u64,
    pending: Vec<u8>,
    skipping_oversized: bool,
    bom_consumed: bool,
    head: Vec<u8>,
}

impl FileCursor {
    pub(crate) fn new() -> Self {
        Self {
            identity: None,
            offset: 0,
            pending: Vec::new(),
            skipping_oversized: false,
            bom_consumed: false,
            head: Vec::new(),
        }
    }

    pub(crate) fn reset_parse_state(&mut self) {
        self.identity = None;
        self.offset = 0;
        self.pending.clear();
        self.skipping_oversized = false;
        self.bom_consumed = false;
        self.head.clear();
    }

    #[cfg(test)]
    pub(crate) fn pending_len(&self) -> usize {
        self.pending.len()
    }

    #[cfg(test)]
    pub(crate) fn skipping_oversized(&self) -> bool {
        self.skipping_oversized
    }

    pub(crate) fn head_bytes(&self) -> &[u8] {
        &self.head
    }

    fn capture_head(&mut self, offset_before: u64, data: &[u8]) {
        if offset_before as usize != self.head.len() || self.head.len() >= HEAD_CAPTURE {
            return;
        }
        let need = HEAD_CAPTURE - self.head.len();
        self.head.extend_from_slice(&data[..need.min(data.len())]);
    }

    pub(crate) fn feed(&mut self, data: &[u8], diagnostics: &mut Diagnostics) -> Vec<Vec<u8>> {
        if !self.bom_consumed {
            self.pending.extend_from_slice(data);
            if self.pending.starts_with(&BOM) {
                self.pending.drain(..3);
                self.bom_consumed = true;
            } else if self.pending.len() >= 3
                || self.pending.contains(&b'\n')
                || !BOM.starts_with(self.pending.as_slice())
            {
                self.bom_consumed = true;
            } else {
                return Vec::new();
            }
            let rest = std::mem::take(&mut self.pending);
            return self.feed_lines(&rest, diagnostics);
        }
        self.feed_lines(data, diagnostics)
    }

    fn feed_lines(&mut self, mut data: &[u8], diagnostics: &mut Diagnostics) -> Vec<Vec<u8>> {
        let mut complete = Vec::new();
        loop {
            if self.skipping_oversized {
                match data.iter().position(|&byte| byte == b'\n') {
                    Some(pos) => {
                        data = &data[pos + 1..];
                        self.skipping_oversized = false;
                        continue;
                    }
                    None => return complete,
                }
            }

            match data.iter().position(|&byte| byte == b'\n') {
                Some(pos) => {
                    let line_chunk = &data[..pos];
                    if self.pending.len().saturating_add(line_chunk.len()) > MAX_PHYSICAL_LINE {
                        diagnostics.oversized_lines += 1;
                        self.pending.clear();
                    } else {
                        self.pending.extend_from_slice(line_chunk);
                        let mut line = std::mem::take(&mut self.pending);
                        if line.last() == Some(&b'\r') {
                            line.pop();
                        }
                        if !line.is_empty() {
                            complete.push(line);
                        }
                    }
                    data = &data[pos + 1..];
                }
                None => {
                    if data.is_empty() {
                        break;
                    }
                    if self.pending.len().saturating_add(data.len()) > MAX_PHYSICAL_LINE {
                        diagnostics.oversized_lines += 1;
                        self.pending.clear();
                        self.skipping_oversized = true;
                    } else {
                        self.pending.extend_from_slice(data);
                    }
                    break;
                }
            }
        }
        complete
    }
}

pub(crate) fn open_shared_read(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(path)
}

pub(crate) fn head_mismatch(file: &mut File, head: &[u8]) -> io::Result<bool> {
    if head.is_empty() {
        return Ok(false);
    }
    file.seek(SeekFrom::Start(0))?;
    let mut buf = vec![0_u8; head.len()];
    let read = file.read(&mut buf)?;
    Ok(read != head.len() || buf != head)
}

pub(crate) fn ingest_from(
    file: &mut File,
    cursor: &mut FileCursor,
    size: u64,
    diagnostics: &mut Diagnostics,
) -> io::Result<Vec<Vec<u8>>> {
    if cursor.offset > size {
        return Ok(Vec::new());
    }
    file.seek(SeekFrom::Start(cursor.offset))?;
    let mut buf = [0_u8; READ_CHUNK];
    let mut complete = Vec::new();
    while cursor.offset < size {
        let remaining = (size - cursor.offset) as usize;
        let to_read = remaining.min(buf.len());
        let read = file.read(&mut buf[..to_read])?;
        if read == 0 {
            break;
        }
        let offset_before = cursor.offset;
        cursor.offset += read as u64;
        cursor.capture_head(offset_before, &buf[..read]);
        complete.extend(cursor.feed(&buf[..read], diagnostics));
    }
    Ok(complete)
}

#[cfg(test)]
pub(crate) use windows_sys::Win32::Storage::FileSystem::{
    FILE_SHARE_DELETE as SHARE_DELETE, FILE_SHARE_READ as SHARE_READ,
    FILE_SHARE_WRITE as SHARE_WRITE,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retains_incomplete_bytes_until_a_physical_line_completes() {
        let mut cursor = FileCursor::new();
        let mut diagnostics = Diagnostics::default();
        assert!(cursor.feed(b"{\"a\":", &mut diagnostics).is_empty());
        assert_eq!(cursor.pending_len(), 5);

        let lines = cursor.feed(b"1}\r\n", &mut diagnostics);
        assert_eq!(lines, vec![b"{\"a\":1}".to_vec()]);
        assert_eq!(cursor.pending_len(), 0);
        assert_eq!(diagnostics.oversized_lines, 0);
    }

    #[test]
    fn strips_leading_bom_and_counts_oversized_lines_without_unbounded_pending() {
        let mut cursor = FileCursor::new();
        let mut diagnostics = Diagnostics::default();
        let mut bom_prefix = BOM.to_vec();
        bom_prefix.extend_from_slice(b"{\"ok\":true}\n");
        let lines = cursor.feed(&bom_prefix, &mut diagnostics);
        assert_eq!(lines, vec![b"{\"ok\":true}".to_vec()]);

        let chunk = vec![b'x'; 64 * 1024];
        for _ in 0..129 {
            assert!(cursor.feed(&chunk, &mut diagnostics).is_empty());
        }
        assert_eq!(diagnostics.oversized_lines, 1);
        assert_eq!(cursor.pending_len(), 0);
        assert!(cursor.skipping_oversized());

        let after = cursor.feed(b"\nsecond\n", &mut diagnostics);
        assert_eq!(after, vec![b"second".to_vec()]);
        assert!(!cursor.skipping_oversized());
    }
}
