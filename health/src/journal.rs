use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use crate::fsutil;
use crate::schema::{
    HealthError, HealthRecord, QueryDiagnostics, MAX_JOURNAL_LINE, SCHEMA_VERSION,
};

#[derive(Clone, Debug, Default)]
pub struct JournalLoad {
    pub records: Vec<HealthRecord>,
    pub diagnostics: QueryDiagnostics,
}

pub fn load(path: &Path) -> Result<JournalLoad, HealthError> {
    let mut loaded = JournalLoad::default();
    let file = match fsutil::open_shared_read(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(loaded),
        Err(error) => return Err(error.into()),
    };
    read_records(file, &mut loaded)?;
    Ok(loaded)
}

fn read_records(file: File, loaded: &mut JournalLoad) -> Result<(), HealthError> {
    let mut reader = BufReader::new(file);
    loop {
        let mut buf = Vec::new();
        let read = reader.read_until(b'\n', &mut buf)?;
        if read == 0 {
            break;
        }
        let had_newline = buf.ends_with(&[b'\n']);
        if !had_newline {
            loaded.diagnostics.journal_incomplete_trailing = true;
            break;
        }
        if buf.starts_with(&[0xEF, 0xBB, 0xBF]) && loaded.records.is_empty() {
            buf.drain(..3);
        }
        if buf.len() > MAX_JOURNAL_LINE {
            loaded.diagnostics.oversized_journal_lines += 1;
            continue;
        }
        while buf.last() == Some(&b'\n') || buf.last() == Some(&b'\r') {
            buf.pop();
        }
        if buf.is_empty() {
            continue;
        }
        match std::str::from_utf8(&buf) {
            Ok(line) => ingest_line(line, loaded),
            Err(_) => loaded.diagnostics.malformed_journal_lines += 1,
        }
    }
    Ok(())
}

fn ingest_line(line: &str, loaded: &mut JournalLoad) {
    match serde_json::from_str::<HealthRecord>(line) {
        Ok(record) if record.schema_version == SCHEMA_VERSION => loaded.records.push(record),
        Ok(_) => loaded.diagnostics.unsupported_journal_records += 1,
        Err(_) => loaded.diagnostics.malformed_journal_lines += 1,
    }
}

pub fn append(path: &Path, record: &HealthRecord) -> Result<(), HealthError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut encoded = serde_json::to_vec(record)?;
    if encoded.len() > MAX_JOURNAL_LINE {
        return Err(HealthError::msg("journal record exceeds bound"));
    }
    encoded.push(b'\n');
    let mut file = fsutil::open_shared_append(path)?;
    file.write_all(&encoded)?;
    file.flush()?;
    file.sync_data()?;
    Ok(())
}
