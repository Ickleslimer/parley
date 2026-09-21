use std::thread;
use std::time::Duration;

use crate::fsutil;
use crate::paths::HealthPaths;
use crate::schema::{
    now_ms, HealthError, QueryDocument, UnavailableReason, MAX_SNAPSHOT_BYTES, SCHEMA_VERSION,
    SNAPSHOT_RETRY_COUNT, SNAPSHOT_RETRY_MS,
};

pub fn write(paths: &HealthPaths, document: &QueryDocument) -> Result<(), HealthError> {
    let bytes = serde_json::to_vec_pretty(document)?;
    if bytes.len() > MAX_SNAPSHOT_BYTES {
        return Err(HealthError::msg("snapshot exceeds bound"));
    }
    fsutil::atomic_write(&paths.snapshot(), &bytes)
}

pub fn read_with_retry(paths: &HealthPaths) -> QueryDocument {
    let generated_ms = now_ms();
    let mut locked = false;
    for attempt in 0..SNAPSHOT_RETRY_COUNT {
        match read_once(paths) {
            Ok(document) => return document,
            Err(ReadFailure::Missing) => {
                if attempt + 1 == SNAPSHOT_RETRY_COUNT {
                    return QueryDocument::unavailable(UnavailableReason::Missing, generated_ms);
                }
            }
            Err(ReadFailure::Locked) => {
                locked = true;
                if attempt + 1 == SNAPSHOT_RETRY_COUNT {
                    return QueryDocument::unavailable(UnavailableReason::Locked, generated_ms);
                }
            }
            Err(ReadFailure::Malformed) => {
                return QueryDocument::unavailable(UnavailableReason::Malformed, generated_ms);
            }
        }
        thread::sleep(Duration::from_millis(SNAPSHOT_RETRY_MS));
    }
    let reason = if locked {
        UnavailableReason::Locked
    } else {
        UnavailableReason::Missing
    };
    QueryDocument::unavailable(reason, generated_ms)
}

enum ReadFailure {
    Missing,
    Locked,
    Malformed,
}

fn read_once(paths: &HealthPaths) -> Result<QueryDocument, ReadFailure> {
    let read = match fsutil::read_bounded(&paths.snapshot(), MAX_SNAPSHOT_BYTES + 1) {
        Ok(read) => read,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ReadFailure::Missing)
        }
        Err(error) if fsutil::is_locked_error(&error) => return Err(ReadFailure::Locked),
        Err(_) => return Err(ReadFailure::Malformed),
    };
    if read.clipped {
        return Err(ReadFailure::Malformed);
    }
    let document: QueryDocument = serde_json::from_slice(fsutil::strip_bom(&read.bytes))
        .map_err(|_| ReadFailure::Malformed)?;
    if document.schema_version != SCHEMA_VERSION {
        return Err(ReadFailure::Malformed);
    }
    if document.unavailable.is_some() {
        return Ok(document);
    }
    Ok(document)
}
