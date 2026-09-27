use std::fs::File;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use super::keys::{decode_key, KeyContext, KeyKind};
use super::parse::{parse_record, ParseOutcome};
use super::source::{
    head_mismatch, ingest_from, open_shared_read, path_identity, FileCursor, FileIdentity,
};
use super::store::{page, Store};
use super::types::{
    Diagnostics, EngineStatus, EventContent, ExchangePage, ExchangeSummary, IdMatch, SearchHit,
    SearchPage, SessionPage, SessionSummary, SourceState, SourceStatus, WidgetExchange,
    WidgetSnapshot,
};

#[derive(Debug)]
pub struct EventEngine {
    inner: Mutex<Inner>,
}

#[derive(Debug)]
struct Inner {
    sources: Vec<WatchedSource>,
}

#[derive(Debug)]
struct WatchedSource {
    configured_path: PathBuf,
    path_display: String,
    identity: String,
    source_state: SourceState,
    generation: u64,
    diagnostics: Diagnostics,
    cursor: FileCursor,
    store: Store,
    alias_of: Option<String>,
    file_identity: Option<FileIdentity>,
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
    identities: Vec<(String, SourceState, u64, Option<String>)>,
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
        self.set_sources(source.into_iter().collect())
    }

    pub fn set_sources(&self, sources: Vec<PathBuf>) -> Result<(), String> {
        for path in &sources {
            if !path.is_absolute() {
                return Err(format!(
                    "event log path must be absolute: {}",
                    path.display()
                ));
            }
        }
        self.lock().reset_sources(sources);
        Ok(())
    }

    pub fn add_source(&self, source: PathBuf) -> Result<(), String> {
        if !source.is_absolute() {
            return Err(format!(
                "event log path must be absolute: {}",
                source.display()
            ));
        }
        self.lock().add_source(source);
        Ok(())
    }

    pub fn remove_source(&self, source: &Path) -> Result<(), String> {
        if !source.is_absolute() {
            return Err(format!(
                "event log path must be absolute: {}",
                source.display()
            ));
        }
        self.lock().remove_source(source);
        Ok(())
    }

    pub fn configured_paths(&self) -> Vec<PathBuf> {
        self.lock()
            .sources
            .iter()
            .map(|source| source.configured_path.clone())
            .collect()
    }

    pub fn poll(&self) -> bool {
        self.lock().poll()
    }

    pub fn status(&self) -> EngineStatus {
        self.lock().status()
    }

    pub fn source_statuses(&self) -> Vec<SourceStatus> {
        self.lock().source_statuses()
    }

    pub fn session_page(&self, cursor: Option<u64>, limit: usize) -> SessionPage {
        self.lock().session_page(cursor, limit)
    }

    pub fn exchange_page(
        &self,
        session_key: &str,
        cursor: Option<u64>,
        limit: usize,
    ) -> ExchangePage {
        self.lock().exchange_page(session_key, cursor, limit)
    }

    pub fn search(&self, query: &str, cursor: Option<u64>, limit: usize) -> SearchPage {
        self.lock().search(query, cursor, limit)
    }

    pub fn event_content(&self, event_key: &str) -> Option<EventContent> {
        self.lock().event_content(event_key)
    }

    pub fn response_for_exchange(&self, exchange_key: &str) -> Option<EventContent> {
        self.lock().response_for_exchange(exchange_key)
    }

    pub fn latest_grok_response_before(
        &self,
        timestamp_ms: u64,
        session_key: Option<&str>,
    ) -> Option<EventContent> {
        self.lock()
            .latest_grok_response_before(timestamp_ms, session_key)
    }

    pub fn match_event_id(&self, event_id: &str) -> IdMatch<EventContent> {
        self.lock().match_event_id(event_id)
    }

    pub fn match_exchange_response(&self, exchange_id: &str) -> IdMatch<EventContent> {
        self.lock().match_exchange_response(exchange_id)
    }

    pub fn match_session_id(&self, session_id: &str) -> IdMatch<String> {
        self.lock().match_session_id(session_id)
    }

    pub fn match_latest_grok_response_before(
        &self,
        timestamp_ms: u64,
        session_id: Option<&str>,
    ) -> IdMatch<EventContent> {
        self.lock()
            .match_latest_grok_response_before(timestamp_ms, session_id)
    }

    pub fn widget_snapshot(&self) -> WidgetSnapshot {
        self.lock().widget_snapshot()
    }

    pub fn widget_exchanges(&self) -> Vec<WidgetExchange> {
        self.lock().widget_exchanges()
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
            sources: Vec::new(),
        }
    }

    fn reset_sources(&mut self, sources: Vec<PathBuf>) {
        self.sources = sources.into_iter().map(WatchedSource::new).collect();
        self.refresh_aliases();
    }

    fn add_source(&mut self, source: PathBuf) {
        self.sources.push(WatchedSource::new(source));
        self.refresh_aliases();
    }

    fn remove_source(&mut self, source: &Path) {
        let needle = path_identity(source);
        self.sources
            .retain(|item| path_identity(&item.configured_path) != needle);
        self.refresh_aliases();
    }

    fn refresh_aliases(&mut self) {
        let snapshot: Vec<(String, Option<FileIdentity>)> = self
            .sources
            .iter()
            .map(|source| (source.identity.clone(), source.file_identity))
            .collect();
        for index in 0..self.sources.len() {
            let mut alias_of = None;
            for earlier in 0..index {
                if sources_alias(
                    &snapshot[index].0,
                    snapshot[index].1,
                    &snapshot[earlier].0,
                    snapshot[earlier].1,
                ) {
                    alias_of = Some(snapshot[earlier].0.clone());
                    break;
                }
            }
            let source = &mut self.sources[index];
            let was_alias = source.alias_of.is_some();
            source.alias_of = alias_of.clone();
            if alias_of.is_some() {
                source.diagnostics.alias_collisions = 1;
                source.diagnostics.last_error = Some(format!(
                    "configured path aliases {}",
                    source.alias_of.as_deref().unwrap_or_default()
                ));
                if !was_alias {
                    source.store = Store::new();
                    source.cursor = FileCursor::new();
                    source.generation = 0;
                }
            } else {
                source.diagnostics.alias_collisions = 0;
                if was_alias {
                    source.diagnostics.last_error = None;
                    source.begin_generation();
                }
            }
        }
    }

    fn observe(&self) -> Observation {
        Observation {
            source_state: aggregate_state(
                &self
                    .unique_sources()
                    .map(|source| source.source_state)
                    .collect::<Vec<_>>(),
            ),
            generation: self.unique_sources().map(|source| source.generation).sum(),
            bytes_read: self
                .unique_sources()
                .map(|source| source.cursor.offset)
                .sum(),
            session_count: self
                .unique_sources()
                .map(|source| source.store.session_count())
                .sum(),
            exchange_count: self
                .unique_sources()
                .map(|source| source.store.exchange_count())
                .sum(),
            event_count: self
                .unique_sources()
                .map(|source| source.store.event_count())
                .sum(),
            last_event_timestamp_ms: self
                .unique_sources()
                .filter_map(|source| source.store.last_event_timestamp_ms())
                .max(),
            diagnostics: self.merged_diagnostics(),
            identities: self
                .sources
                .iter()
                .map(|source| {
                    (
                        source.identity.clone(),
                        source.source_state,
                        source.generation,
                        source.alias_of.clone(),
                    )
                })
                .collect(),
        }
    }

    fn unique_sources(&self) -> impl Iterator<Item = &WatchedSource> {
        self.sources
            .iter()
            .filter(|source| source.alias_of.is_none())
    }

    fn unique_sources_enumerated(&self) -> impl Iterator<Item = (usize, &WatchedSource)> {
        self.sources
            .iter()
            .enumerate()
            .filter(|(_, source)| source.alias_of.is_none())
    }

    fn merged_diagnostics(&self) -> Diagnostics {
        let mut diagnostics = Diagnostics::default();
        for source in &self.sources {
            diagnostics.merge_from(&source.diagnostics);
        }
        diagnostics
    }

    fn status(&self) -> EngineStatus {
        let unique: Vec<&WatchedSource> = self.unique_sources().collect();
        EngineStatus {
            source_state: aggregate_state(
                &unique
                    .iter()
                    .map(|source| source.source_state)
                    .collect::<Vec<_>>(),
            ),
            generation: unique.iter().map(|source| source.generation).sum(),
            bytes_read: unique.iter().map(|source| source.cursor.offset).sum(),
            session_count: unique
                .iter()
                .map(|source| source.store.session_count())
                .sum(),
            exchange_count: unique
                .iter()
                .map(|source| source.store.exchange_count())
                .sum(),
            last_event_timestamp_ms: unique
                .iter()
                .filter_map(|source| source.store.last_event_timestamp_ms())
                .max(),
            diagnostics: self.merged_diagnostics(),
            sources: self.source_statuses(),
        }
    }

    fn source_statuses(&self) -> Vec<SourceStatus> {
        self.sources
            .iter()
            .map(|source| SourceStatus {
                path: source.configured_path.to_string_lossy().into_owned(),
                identity: source.identity.clone(),
                source_state: source.source_state,
                generation: source.generation,
                bytes_read: source.cursor.offset,
                session_count: source.store.session_count(),
                exchange_count: source.store.exchange_count(),
                last_event_timestamp_ms: source.store.last_event_timestamp_ms(),
                diagnostics: source.diagnostics.clone(),
                alias_of: source.alias_of.clone(),
            })
            .collect()
    }

    fn poll(&mut self) -> bool {
        let before = self.observe();
        if self.sources.is_empty() {
            return self.observe() != before;
        }
        for index in 0..self.sources.len() {
            if self.sources[index].alias_of.is_some() {
                continue;
            }
            self.poll_source(index);
        }
        self.refresh_aliases();
        self.observe() != before
    }

    fn poll_source(&mut self, index: usize) {
        let path = self.sources[index].configured_path.clone();
        match open_shared_read(&path) {
            Ok(mut file) => {
                if let Err(error) = self.poll_open_file(index, &mut file) {
                    let source = &mut self.sources[index];
                    source.note_io(&error);
                    source.source_state = SourceState::Degraded;
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let source = &mut self.sources[index];
                source.source_state = SourceState::Missing;
                source.file_identity = None;
            }
            Err(error) => {
                let source = &mut self.sources[index];
                source.note_io(&error);
                source.source_state = SourceState::Degraded;
            }
        }
    }

    fn poll_open_file(&mut self, index: usize, file: &mut File) -> io::Result<()> {
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            let source = &mut self.sources[index];
            source.source_state = SourceState::Degraded;
            source.diagnostics.io_errors += 1;
            source.diagnostics.last_error = Some("source path is not a regular file".to_string());
            return Ok(());
        }
        let identity = FileIdentity::from_file(file)?;
        let size = metadata.len();
        let source = &mut self.sources[index];
        source.identity = path_identity(&source.configured_path);
        source.file_identity = Some(identity);
        let replaced = source
            .cursor
            .identity
            .map(|old| old != identity)
            .unwrap_or(true);
        let truncated = source.cursor.identity == Some(identity) && size < source.cursor.offset;
        let rewritten = if !replaced && !truncated && source.generation > 0 {
            head_mismatch(file, source.cursor.head_bytes())?
        } else {
            false
        };
        if replaced || truncated || rewritten {
            source.begin_generation();
            source.cursor.identity = Some(identity);
        }
        source.source_state = SourceState::Watching;
        let lines = ingest_from(file, &mut source.cursor, size, &mut source.diagnostics)?;
        for line in lines {
            source.consume_line(&line);
        }
        Ok(())
    }

    fn session_page(&self, cursor: Option<u64>, limit: usize) -> SessionPage {
        let mut ranked: Vec<(u64, usize, String, SessionSummary)> = Vec::new();
        for (order, source) in self.unique_sources_enumerated() {
            for session in source.store.all_session_summaries(source.keys()) {
                ranked.push((
                    session.latest_timestamp_ms,
                    order,
                    session.session_key.clone(),
                    session,
                ));
            }
        }
        ranked.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| left.1.cmp(&right.1))
                .then_with(|| left.2.cmp(&right.2))
        });
        page(
            ranked.into_iter().map(|item| item.3).collect(),
            cursor,
            limit,
            |items, next, total| SessionPage {
                items,
                next_cursor: next,
                total,
            },
        )
    }

    fn exchange_page(&self, session_key: &str, cursor: Option<u64>, limit: usize) -> ExchangePage {
        let Some(decoded) = decode_key(session_key, KeyKind::Session) else {
            return empty_exchange_page();
        };
        let Some((_, source)) = self.source_for_key(&decoded.source_id, decoded.generation) else {
            return empty_exchange_page();
        };
        let mut items = source
            .store
            .all_exchange_summaries(&decoded.raw_id, source.keys());
        items.sort_by(|left, right| {
            right
                .timestamp_ms
                .cmp(&left.timestamp_ms)
                .then_with(|| left.exchange_key.cmp(&right.exchange_key))
        });
        page(items, cursor, limit, |items, next, total| ExchangePage {
            items,
            next_cursor: next,
            total,
        })
    }

    fn search(&self, query: &str, cursor: Option<u64>, limit: usize) -> SearchPage {
        let query = query.trim();
        if query.is_empty() {
            return SearchPage {
                items: Vec::new(),
                next_cursor: None,
                total: 0,
            };
        }
        let mut ranked: Vec<(u64, usize, String, SearchHit)> = Vec::new();
        for (order, source) in self.unique_sources_enumerated() {
            for hit in source.store.all_search_hits(query, source.keys()) {
                ranked.push((hit.timestamp_ms, order, hit.event_key.clone(), hit));
            }
        }
        ranked.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| left.1.cmp(&right.1))
                .then_with(|| left.2.cmp(&right.2))
        });
        page(
            ranked.into_iter().map(|item| item.3).collect(),
            cursor,
            limit,
            |items, next, total| SearchPage {
                items,
                next_cursor: next,
                total,
            },
        )
    }

    fn event_content(&self, event_key: &str) -> Option<EventContent> {
        let decoded = decode_key(event_key, KeyKind::Event)?;
        let (_, source) = self.source_for_key(&decoded.source_id, decoded.generation)?;
        source.store.event_content(&decoded.raw_id, source.keys())
    }

    fn response_for_exchange(&self, exchange_key: &str) -> Option<EventContent> {
        let decoded = decode_key(exchange_key, KeyKind::Exchange)?;
        let (_, source) = self.source_for_key(&decoded.source_id, decoded.generation)?;
        source
            .store
            .response_for_exchange(&decoded.raw_id, source.keys())
    }

    fn latest_grok_response_before(
        &self,
        timestamp_ms: u64,
        session_key: Option<&str>,
    ) -> Option<EventContent> {
        if let Some(session_key) = session_key.filter(|value| !value.is_empty()) {
            let decoded = decode_key(session_key, KeyKind::Session)?;
            let (_, source) = self.source_for_key(&decoded.source_id, decoded.generation)?;
            return source.store.latest_grok_response_before(
                timestamp_ms,
                Some(decoded.raw_id.as_str()),
                source.keys(),
            );
        }
        self.unique_sources_enumerated()
            .filter_map(|(order, source)| {
                source
                    .store
                    .latest_grok_response_before(timestamp_ms, None, source.keys())
                    .map(|content| {
                        (
                            content.timestamp_ms,
                            order,
                            content.event_key.clone(),
                            content,
                        )
                    })
            })
            .max_by(|left, right| {
                left.0
                    .cmp(&right.0)
                    .then_with(|| right.1.cmp(&left.1))
                    .then_with(|| right.2.cmp(&left.2))
            })
            .map(|item| item.3)
    }

    fn match_event_id(&self, event_id: &str) -> IdMatch<EventContent> {
        collect_match(
            self.unique_sources().filter_map(|source| {
                source
                    .store
                    .contains_event(event_id)
                    .then(|| source.store.event_content(event_id, source.keys()))
                    .flatten()
            }),
            event_id,
            "event",
        )
    }

    fn match_exchange_response(&self, exchange_id: &str) -> IdMatch<EventContent> {
        collect_match(
            self.unique_sources().filter_map(|source| {
                source
                    .store
                    .contains_exchange(exchange_id)
                    .then(|| {
                        source
                            .store
                            .response_for_exchange(exchange_id, source.keys())
                    })
                    .flatten()
            }),
            exchange_id,
            "exchange",
        )
    }

    fn match_session_id(&self, session_id: &str) -> IdMatch<String> {
        collect_match(
            self.unique_sources()
                .filter(|source| source.store.contains_session(session_id))
                .map(|source| source.keys().session_key(session_id)),
            session_id,
            "session",
        )
    }

    fn match_latest_grok_response_before(
        &self,
        timestamp_ms: u64,
        session_id: Option<&str>,
    ) -> IdMatch<EventContent> {
        if let Some(session_id) = session_id.filter(|value| !value.is_empty()) {
            match self.match_session_id(session_id) {
                IdMatch::Ambiguous { count, diagnostic } => {
                    return IdMatch::Ambiguous { count, diagnostic };
                }
                IdMatch::None => return IdMatch::None,
                IdMatch::Unique(session_key) => {
                    return self
                        .latest_grok_response_before(timestamp_ms, Some(session_key.as_str()))
                        .map(IdMatch::Unique)
                        .unwrap_or(IdMatch::None);
                }
            }
        }
        self.latest_grok_response_before(timestamp_ms, None)
            .map(IdMatch::Unique)
            .unwrap_or(IdMatch::None)
    }

    fn widget_snapshot(&self) -> WidgetSnapshot {
        let mut ranked: Vec<(u64, usize, String, ExchangeSummary)> = Vec::new();
        for (order, source) in self.unique_sources_enumerated() {
            if let Some(exchange) = source.store.newest_exchange_summary(source.keys()) {
                ranked.push((
                    exchange.timestamp_ms,
                    order,
                    exchange.exchange_key.clone(),
                    exchange,
                ));
            }
        }
        ranked.sort_by(|left, right| {
            compare_newest_first((left.0, left.1, &left.2), (right.0, right.1, &right.2))
        });
        let Some(summary) = ranked.into_iter().next().map(|item| item.3) else {
            return WidgetSnapshot::empty();
        };
        WidgetSnapshot::from_exchange(&summary)
    }

    fn widget_exchanges(&self) -> Vec<WidgetExchange> {
        let mut ranked: Vec<(u64, usize, String, WidgetExchange)> = Vec::new();
        for (order, source) in self.unique_sources_enumerated() {
            for summary in source.store.all_exchanges(source.keys()) {
                ranked.push((
                    summary.timestamp_ms,
                    order,
                    summary.exchange_key.clone(),
                    WidgetExchange {
                        source_identity: source.identity.clone(),
                        summary,
                    },
                ));
            }
        }
        ranked.sort_by(|left, right| {
            compare_newest_first((left.0, left.1, &left.2), (right.0, right.1, &right.2))
        });
        ranked.into_iter().map(|item| item.3).collect()
    }

    fn source_for_key(&self, source_id: &str, generation: u64) -> Option<(usize, &WatchedSource)> {
        self.unique_sources_enumerated()
            .find(|(_, source)| source.identity == source_id && source.generation == generation)
    }
}

impl WatchedSource {
    fn new(path: PathBuf) -> Self {
        let identity = path_identity(&path);
        let path_display = path.to_string_lossy().into_owned();
        Self {
            configured_path: path,
            path_display,
            identity,
            source_state: SourceState::Missing,
            generation: 0,
            diagnostics: Diagnostics::default(),
            cursor: FileCursor::new(),
            store: Store::new(),
            alias_of: None,
            file_identity: None,
        }
    }

    fn keys(&self) -> KeyContext<'_> {
        KeyContext {
            source_id: &self.identity,
            generation: self.generation,
            path: &self.path_display,
        }
    }

    fn begin_generation(&mut self) {
        self.generation += 1;
        self.cursor.reset_parse_state();
        self.store = Store::new();
    }

    fn note_io(&mut self, error: &io::Error) {
        self.diagnostics.io_errors += 1;
        self.diagnostics.last_error = Some(error.to_string());
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

fn sources_alias(
    left_id: &str,
    left_file: Option<FileIdentity>,
    right_id: &str,
    right_file: Option<FileIdentity>,
) -> bool {
    if left_id == right_id {
        return true;
    }
    match (left_file, right_file) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

fn aggregate_state(states: &[SourceState]) -> SourceState {
    if states.is_empty() {
        return SourceState::None;
    }
    if states.contains(&SourceState::Degraded) {
        return SourceState::Degraded;
    }
    let watching = states.contains(&SourceState::Watching);
    let missing = states.contains(&SourceState::Missing);
    if watching && missing {
        SourceState::Degraded
    } else if watching {
        SourceState::Watching
    } else if missing {
        SourceState::Missing
    } else {
        SourceState::None
    }
}

fn compare_newest_first(left: (u64, usize, &str), right: (u64, usize, &str)) -> std::cmp::Ordering {
    right
        .0
        .cmp(&left.0)
        .then_with(|| left.1.cmp(&right.1))
        .then_with(|| left.2.cmp(&right.2))
}

fn empty_exchange_page() -> ExchangePage {
    ExchangePage {
        items: Vec::new(),
        next_cursor: None,
        total: 0,
    }
}

fn collect_match<T, I>(matches: I, raw_id: &str, kind: &str) -> IdMatch<T>
where
    I: IntoIterator<Item = T>,
{
    let found: Vec<T> = matches.into_iter().collect();
    match found.len() {
        0 => IdMatch::None,
        1 => IdMatch::Unique(found.into_iter().next().expect("len 1")),
        count => IdMatch::Ambiguous {
            count,
            diagnostic: format!(
                "ambiguous {kind} id '{raw_id}' matched {count} sources; no selection made"
            ),
        },
    }
}
