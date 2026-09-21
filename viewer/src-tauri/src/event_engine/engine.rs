use std::fs::File;
use std::io::{self, ErrorKind};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use super::parse::{parse_record, ParseOutcome};
use super::source::{head_mismatch, ingest_from, open_shared_read, FileCursor, FileIdentity};
use super::store::Store;
use super::types::{
    Diagnostics, EngineStatus, EventContent, ExchangePage, SearchPage, SessionPage, SourceState,
    WidgetSnapshot,
};

#[derive(Debug)]
pub struct EventEngine {
    inner: Mutex<Inner>,
}

#[derive(Debug)]
struct Inner {
    source: Option<PathBuf>,
    source_state: SourceState,
    generation: u64,
    diagnostics: Diagnostics,
    cursor: FileCursor,
    store: Store,
}

#[derive(Clone, PartialEq, Eq)]
struct Observation {
    source_state: SourceState,
    generation: u64,
    bytes_read: u64,
    session_count: u64,
    exchange_count: u64,
    event_count: usize,
    last_event_timestamp_ms: Option<u64>,
    diagnostics: Diagnostics,
}

impl EventEngine {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner::new()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn set_source(&self, source: Option<PathBuf>) -> Result<(), String> {
        if let Some(path) = source.as_ref() {
            if !path.is_absolute() {
                return Err(format!(
                    "event log path must be absolute: {}",
                    path.display()
                ));
            }
        }
        self.lock().reset_source(source);
        Ok(())
    }

    pub fn poll(&self) -> bool {
        self.lock().poll()
    }

    pub fn status(&self) -> EngineStatus {
        self.lock().status()
    }

    pub fn session_page(&self, cursor: Option<u64>, limit: usize) -> SessionPage {
        self.lock().store.session_page(cursor, limit)
    }

    pub fn exchange_page(
        &self,
        session_id: &str,
        cursor: Option<u64>,
        limit: usize,
    ) -> ExchangePage {
        self.lock().store.exchange_page(session_id, cursor, limit)
    }

    pub fn search(&self, query: &str, cursor: Option<u64>, limit: usize) -> SearchPage {
        self.lock().store.search(query, cursor, limit)
    }

    pub fn event_content(&self, event_id: &str) -> Option<EventContent> {
        self.lock().store.event_content(event_id)
    }

    pub fn response_for_exchange(&self, exchange_id: &str) -> Option<EventContent> {
        self.lock().store.response_for_exchange(exchange_id)
    }

    pub fn latest_grok_response_before(
        &self,
        timestamp_ms: u64,
        session_id: Option<&str>,
    ) -> Option<EventContent> {
        self.lock()
            .store
            .latest_grok_response_before(timestamp_ms, session_id)
    }

    pub fn widget_snapshot(&self) -> WidgetSnapshot {
        self.lock().store.widget_snapshot()
    }
}

impl Default for EventEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl Inner {
    fn new() -> Self {
        Self {
            source: None,
            source_state: SourceState::None,
            generation: 0,
            diagnostics: Diagnostics::default(),
            cursor: FileCursor::new(),
            store: Store::new(),
        }
    }

    fn reset_source(&mut self, source: Option<PathBuf>) {
        self.source = source;
        self.source_state = if self.source.is_some() {
            SourceState::Missing
        } else {
            SourceState::None
        };
        self.generation = 0;
        self.diagnostics = Diagnostics::default();
        self.cursor = FileCursor::new();
        self.store = Store::new();
    }

    fn begin_generation(&mut self) {
        self.generation += 1;
        self.cursor.reset_parse_state();
        self.store = Store::new();
    }

    fn observe(&self) -> Observation {
        Observation {
            source_state: self.source_state,
            generation: self.generation,
            bytes_read: self.cursor.offset,
            session_count: self.store.session_count(),
            exchange_count: self.store.exchange_count(),
            event_count: self.store.event_count(),
            last_event_timestamp_ms: self.store.last_event_timestamp_ms(),
            diagnostics: self.diagnostics.clone(),
        }
    }

    fn status(&self) -> EngineStatus {
        EngineStatus {
            source_path: self
                .source
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            source_state: self.source_state,
            generation: self.generation,
            bytes_read: self.cursor.offset,
            session_count: self.store.session_count(),
            exchange_count: self.store.exchange_count(),
            last_event_timestamp_ms: self.store.last_event_timestamp_ms(),
            diagnostics: self.diagnostics.clone(),
        }
    }

    fn note_io(&mut self, error: &io::Error) {
        self.diagnostics.io_errors += 1;
        self.diagnostics.last_error = Some(error.to_string());
    }

    fn poll(&mut self) -> bool {
        let before = self.observe();
        let Some(path) = self.source.clone() else {
            self.source_state = SourceState::None;
            return self.observe() != before;
        };

        match open_shared_read(&path) {
            Ok(mut file) => {
                if let Err(error) = self.poll_open_file(&mut file) {
                    self.note_io(&error);
                    self.source_state = SourceState::Degraded;
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                self.source_state = SourceState::Missing;
            }
            Err(error) => {
                self.note_io(&error);
                self.source_state = SourceState::Degraded;
            }
        }
        self.observe() != before
    }

    fn poll_open_file(&mut self, file: &mut File) -> io::Result<()> {
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            self.source_state = SourceState::Degraded;
            self.diagnostics.io_errors += 1;
            self.diagnostics.last_error = Some("source path is not a regular file".to_string());
            return Ok(());
        }
        let identity = FileIdentity::from_file(file)?;
        let size = metadata.len();
        let replaced = self
            .cursor
            .identity
            .map(|old| old != identity)
            .unwrap_or(true);
        let truncated = self.cursor.identity == Some(identity) && size < self.cursor.offset;
        let rewritten = if !replaced && !truncated && self.generation > 0 {
            head_mismatch(file, self.cursor.head_bytes())?
        } else {
            false
        };
        if replaced || truncated || rewritten {
            self.begin_generation();
            self.cursor.identity = Some(identity);
        }
        self.source_state = SourceState::Watching;
        let lines = ingest_from(file, &mut self.cursor, size, &mut self.diagnostics)?;
        for line in lines {
            self.consume_line(&line);
        }
        Ok(())
    }

    fn consume_line(&mut self, bytes: &[u8]) {
        let text = match std::str::from_utf8(bytes) {
            Ok(text) => text,
            Err(_) => {
                self.diagnostics.malformed_lines += 1;
                return;
            }
        };
        match parse_record(text) {
            Ok(parsed) => {
                if !self.store.insert(parsed) {
                    self.diagnostics.duplicate_events += 1;
                }
            }
            Err(ParseOutcome::Malformed) => self.diagnostics.malformed_lines += 1,
            Err(ParseOutcome::Unsupported) => self.diagnostics.unsupported_records += 1,
        }
    }
}
