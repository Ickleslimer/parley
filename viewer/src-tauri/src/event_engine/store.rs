use std::collections::HashMap;

use super::excerpt::{excerpt, parse_context_diagnostics, widget_projection, window_from};
use super::keys::KeyContext;
use super::parse::ParsedEvent;
use super::types::{
    clamp_page_limit, speakers, EventContent, EventType, ExchangeSummary, MessagePreview,
    SearchHit, SessionSummary, WidgetFeedExchange, WidgetFeedMessage, PENDING_LABEL,
    WIDGET_FEED_BODY_LIMIT,
};

#[derive(Clone, Debug)]
struct StoredEvent {
    event_id: String,
    exchange_id: String,
    session_id: String,
    event_type: EventType,
    source: String,
    target: String,
    timestamp_ms: u64,
    status: String,
    duration_ms: Option<u64>,
    error: Option<String>,
    content: String,
    seq: u64,
}

#[derive(Clone, Debug)]
struct ExchangeAcc {
    exchange_id: String,
    session_id: String,
    request_idx: Option<usize>,
    completion_idx: Option<usize>,
    timestamp_ms: u64,
    last_seq: u64,
}

#[derive(Clone, Debug)]
struct SessionAcc {
    session_id: String,
    exchange_count: u64,
    latest_timestamp_ms: u64,
    latest_exchange_id: String,
    latest_seq: u64,
}

#[derive(Debug, Default)]
pub(crate) struct Store {
    events: Vec<StoredEvent>,
    by_event_id: HashMap<String, usize>,
    exchanges: HashMap<String, ExchangeAcc>,
    sessions: HashMap<String, SessionAcc>,
    next_seq: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WidgetFeedRank {
    pub exchange_id: String,
    pub exchange_key: String,
    pub timestamp_ms: u64,
}

impl Store {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn event_count(&self) -> usize {
        self.events.len()
    }

    pub(crate) fn session_count(&self) -> u64 {
        self.sessions.len() as u64
    }

    pub(crate) fn exchange_count(&self) -> u64 {
        self.exchanges.len() as u64
    }

    pub(crate) fn last_event_timestamp_ms(&self) -> Option<u64> {
        self.events.iter().map(|event| event.timestamp_ms).max()
    }

    pub(crate) fn contains_event(&self, event_id: &str) -> bool {
        self.by_event_id.contains_key(event_id)
    }

    pub(crate) fn contains_exchange(&self, exchange_id: &str) -> bool {
        self.exchanges.contains_key(exchange_id)
    }

    pub(crate) fn contains_session(&self, session_id: &str) -> bool {
        self.sessions.contains_key(session_id)
    }

    pub(crate) fn insert(&mut self, parsed: ParsedEvent) -> bool {
        if self.by_event_id.contains_key(&parsed.event_id) {
            return false;
        }
        let session_id = parsed.session_id.unwrap_or_default();
        let idx = self.events.len();
        let seq = self.next_seq;
        self.next_seq += 1;
        let stored = StoredEvent {
            event_id: parsed.event_id,
            exchange_id: parsed.exchange_id,
            session_id,
            event_type: parsed.event_type,
            source: parsed.source,
            target: parsed.target,
            timestamp_ms: parsed.timestamp_ms,
            status: parsed.status,
            duration_ms: parsed.duration_ms,
            error: parsed.error,
            content: parsed.content.unwrap_or_default(),
            seq,
        };
        self.by_event_id.insert(stored.event_id.clone(), idx);
        self.upsert_exchange(&stored, idx);
        self.upsert_session(&stored);
        self.events.push(stored);
        true
    }

    fn upsert_exchange(&mut self, event: &StoredEvent, idx: usize) {
        let is_new = !self.exchanges.contains_key(&event.exchange_id);
        let acc = self
            .exchanges
            .entry(event.exchange_id.clone())
            .or_insert_with(|| ExchangeAcc {
                exchange_id: event.exchange_id.clone(),
                session_id: event.session_id.clone(),
                request_idx: None,
                completion_idx: None,
                timestamp_ms: event.timestamp_ms,
                last_seq: event.seq,
            });
        if event.timestamp_ms > acc.timestamp_ms
            || (event.timestamp_ms == acc.timestamp_ms && event.seq > acc.last_seq)
        {
            acc.timestamp_ms = event.timestamp_ms;
            acc.last_seq = event.seq;
        }
        match event.event_type {
            EventType::Request => {
                if acc.request_idx.is_none() {
                    acc.request_idx = Some(idx);
                }
            }
            EventType::Response | EventType::Error => {
                let replace = match acc.completion_idx {
                    None => true,
                    Some(existing) => {
                        let current = &self.events[existing];
                        event.timestamp_ms > current.timestamp_ms
                            || (event.timestamp_ms == current.timestamp_ms
                                && event.seq > current.seq)
                    }
                };
                if replace {
                    acc.completion_idx = Some(idx);
                }
            }
        }
        if is_new {
            self.sessions
                .entry(event.session_id.clone())
                .or_insert_with(|| SessionAcc {
                    session_id: event.session_id.clone(),
                    exchange_count: 0,
                    latest_timestamp_ms: event.timestamp_ms,
                    latest_exchange_id: event.exchange_id.clone(),
                    latest_seq: event.seq,
                })
                .exchange_count += 1;
        }
    }

    fn upsert_session(&mut self, event: &StoredEvent) {
        let Some(session) = self.sessions.get_mut(&event.session_id) else {
            return;
        };
        if event.timestamp_ms > session.latest_timestamp_ms
            || (event.timestamp_ms == session.latest_timestamp_ms && event.seq > session.latest_seq)
        {
            session.latest_timestamp_ms = event.timestamp_ms;
            session.latest_exchange_id = event.exchange_id.clone();
            session.latest_seq = event.seq;
        }
    }

    pub(crate) fn all_session_summaries(&self, keys: KeyContext<'_>) -> Vec<SessionSummary> {
        self.sessions
            .values()
            .map(|session| self.session_summary(session, keys))
            .collect()
    }

    pub(crate) fn all_exchange_summaries(
        &self,
        session_id: &str,
        keys: KeyContext<'_>,
    ) -> Vec<ExchangeSummary> {
        self.exchanges
            .values()
            .filter(|exchange| exchange.session_id == session_id)
            .map(|exchange| self.exchange_summary(exchange, keys))
            .collect()
    }

    pub(crate) fn widget_feed_ranks(&self, keys: KeyContext<'_>) -> Vec<WidgetFeedRank> {
        self.exchanges
            .values()
            .map(|exchange| WidgetFeedRank {
                exchange_id: exchange.exchange_id.clone(),
                exchange_key: keys.exchange_key(&exchange.exchange_id),
                timestamp_ms: exchange.timestamp_ms,
            })
            .collect()
    }

    pub(crate) fn widget_feed_exchange(
        &self,
        exchange_id: &str,
        keys: KeyContext<'_>,
    ) -> Option<WidgetFeedExchange> {
        let exchange = self.exchanges.get(exchange_id)?;
        let request = exchange
            .request_idx
            .map(|idx| self.widget_feed_message_at(idx, keys, true));
        let completion = exchange
            .completion_idx
            .map(|idx| self.widget_feed_message_at(idx, keys, true));
        Some(WidgetFeedExchange {
            exchange_key: keys.exchange_key(&exchange.exchange_id),
            session_key: keys.session_key(&exchange.session_id),
            timestamp_ms: exchange.timestamp_ms,
            pending_label: (request.is_some() && completion.is_none())
                .then(|| PENDING_LABEL.to_string()),
            request,
            completion,
        })
    }

    pub(crate) fn widget_feed_message(
        &self,
        event_id: &str,
        keys: KeyContext<'_>,
    ) -> Option<WidgetFeedMessage> {
        let idx = *self.by_event_id.get(event_id)?;
        Some(self.widget_feed_message_at(idx, keys, false))
    }

    pub(crate) fn all_search_hits(&self, query: &str, keys: KeyContext<'_>) -> Vec<SearchHit> {
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }
        self.events
            .iter()
            .filter_map(|event| {
                let text = preview_text(event);
                let match_offset = find_ignore_case(text, query)?;
                Some(SearchHit {
                    event_key: keys.event_key(&event.event_id),
                    exchange_key: keys.exchange_key(&event.exchange_id),
                    session_key: keys.session_key(&event.session_id),
                    event_id: event.event_id.clone(),
                    exchange_id: event.exchange_id.clone(),
                    session_id: event.session_id.clone(),
                    source_path: keys.path.to_string(),
                    event_type: event.event_type,
                    timestamp_ms: event.timestamp_ms,
                    excerpt: window_from(text, match_offset),
                    match_offset: match_offset as u64,
                })
            })
            .collect()
    }

    pub(crate) fn event_content(
        &self,
        event_id: &str,
        keys: KeyContext<'_>,
    ) -> Option<EventContent> {
        let idx = *self.by_event_id.get(event_id)?;
        Some(self.content_at(idx, keys))
    }

    pub(crate) fn response_for_exchange(
        &self,
        exchange_id: &str,
        keys: KeyContext<'_>,
    ) -> Option<EventContent> {
        let exchange = self.exchanges.get(exchange_id)?;
        let idx = exchange.completion_idx?;
        (self.events[idx].event_type == EventType::Response).then(|| self.content_at(idx, keys))
    }

    pub(crate) fn latest_grok_response_before(
        &self,
        timestamp_ms: u64,
        session_id: Option<&str>,
        keys: KeyContext<'_>,
    ) -> Option<EventContent> {
        let idx = self
            .events
            .iter()
            .enumerate()
            .filter(|(_, event)| {
                event.event_type == EventType::Response
                    && event.target.eq_ignore_ascii_case("grok")
                    && event.timestamp_ms <= timestamp_ms
                    && session_id
                        .filter(|session_id| !session_id.is_empty())
                        .map(|session_id| event.session_id == session_id)
                        .unwrap_or(true)
            })
            .max_by(|(_, left), (_, right)| {
                left.timestamp_ms
                    .cmp(&right.timestamp_ms)
                    .then_with(|| left.seq.cmp(&right.seq))
            })
            .map(|(idx, _)| idx)?;
        Some(self.content_at(idx, keys))
    }

    fn content_at(&self, idx: usize, keys: KeyContext<'_>) -> EventContent {
        let event = &self.events[idx];
        let (speaker, recipient) = speakers(event.event_type, &event.source, &event.target);
        EventContent {
            event_key: keys.event_key(&event.event_id),
            exchange_key: keys.exchange_key(&event.exchange_id),
            session_key: keys.session_key(&event.session_id),
            event_id: event.event_id.clone(),
            exchange_id: event.exchange_id.clone(),
            session_id: event.session_id.clone(),
            source_path: keys.path.to_string(),
            event_type: event.event_type,
            speaker,
            recipient,
            timestamp_ms: event.timestamp_ms,
            status: event.status.clone(),
            duration_ms: event.duration_ms,
            error: event.error.clone(),
            content: event.content.clone(),
            context: parse_context_diagnostics(&event.content, &event.exchange_id),
        }
    }

    fn session_summary(&self, session: &SessionAcc, keys: KeyContext<'_>) -> SessionSummary {
        let exchange = self.exchanges.get(&session.latest_exchange_id);
        let (latest_source, latest_target, latest_excerpt, excerpt_extracted) = exchange
            .map(|exchange| {
                let preview = exchange
                    .request_idx
                    .map(|idx| self.preview(&self.events[idx], keys))
                    .or_else(|| {
                        exchange
                            .completion_idx
                            .map(|idx| self.preview(&self.events[idx], keys))
                    });
                let source_target = exchange
                    .request_idx
                    .or(exchange.completion_idx)
                    .map(|idx| {
                        let event = &self.events[idx];
                        (event.source.clone(), event.target.clone())
                    })
                    .unwrap_or_default();
                match preview {
                    Some(preview) => (
                        source_target.0,
                        source_target.1,
                        preview.excerpt,
                        preview.excerpt_extracted,
                    ),
                    None => (source_target.0, source_target.1, String::new(), false),
                }
            })
            .unwrap_or_default();
        SessionSummary {
            session_key: keys.session_key(&session.session_id),
            session_id: session.session_id.clone(),
            source_path: keys.path.to_string(),
            exchange_count: session.exchange_count,
            latest_timestamp_ms: session.latest_timestamp_ms,
            latest_source,
            latest_target,
            latest_excerpt,
            excerpt_extracted,
        }
    }

    fn exchange_summary(&self, exchange: &ExchangeAcc, keys: KeyContext<'_>) -> ExchangeSummary {
        let request = exchange
            .request_idx
            .map(|idx| self.preview(&self.events[idx], keys));
        let completion = exchange
            .completion_idx
            .map(|idx| self.preview(&self.events[idx], keys));
        let pending_label = if request.is_some() && completion.is_none() {
            Some(PENDING_LABEL.to_string())
        } else {
            None
        };
        ExchangeSummary {
            exchange_key: keys.exchange_key(&exchange.exchange_id),
            session_key: keys.session_key(&exchange.session_id),
            exchange_id: exchange.exchange_id.clone(),
            session_id: exchange.session_id.clone(),
            source_path: keys.path.to_string(),
            timestamp_ms: exchange.timestamp_ms,
            request,
            completion,
            pending_label,
        }
    }

    fn preview(&self, event: &StoredEvent, keys: KeyContext<'_>) -> MessagePreview {
        let (speaker, recipient) = speakers(event.event_type, &event.source, &event.target);
        let text = preview_text(event);
        let (excerpt, excerpt_extracted) = excerpt(
            text,
            event.event_type == EventType::Request,
            &event.exchange_id,
        );
        MessagePreview {
            event_key: keys.event_key(&event.event_id),
            event_id: event.event_id.clone(),
            event_type: event.event_type,
            speaker,
            recipient,
            timestamp_ms: event.timestamp_ms,
            status: event.status.clone(),
            excerpt,
            excerpt_extracted,
            content_length: text.chars().count() as u64,
        }
    }

    fn widget_feed_message_at(
        &self,
        idx: usize,
        keys: KeyContext<'_>,
        bounded: bool,
    ) -> WidgetFeedMessage {
        let event = &self.events[idx];
        let (speaker, recipient) = speakers(event.event_type, &event.source, &event.target);
        let projected =
            widget_projection(preview_text(event), event.event_type, &event.exchange_id);
        let full_character_length = projected.text.chars().count();
        let truncated = bounded && full_character_length > WIDGET_FEED_BODY_LIMIT;
        let body = if bounded {
            projected
                .text
                .chars()
                .take(WIDGET_FEED_BODY_LIMIT)
                .collect()
        } else {
            projected.text.to_string()
        };
        WidgetFeedMessage {
            event_key: keys.event_key(&event.event_id),
            event_type: event.event_type,
            speaker,
            recipient,
            timestamp_ms: event.timestamp_ms,
            status: event.status.clone(),
            body,
            full_character_length: full_character_length as u64,
            truncated,
            projection: projected.projection,
            context_omitted: projected.context_omitted,
        }
    }
}

fn preview_text(event: &StoredEvent) -> &str {
    if event.event_type == EventType::Error && event.content.is_empty() {
        event.error.as_deref().unwrap_or_default()
    } else {
        &event.content
    }
}

pub(crate) fn page<T: Clone, R>(
    items: Vec<T>,
    cursor: Option<u64>,
    limit: usize,
    finish: impl FnOnce(Vec<T>, Option<u64>, u64) -> R,
) -> R {
    let limit = clamp_page_limit(limit);
    let total = items.len() as u64;
    let start = match usize::try_from(cursor.unwrap_or(0)) {
        Ok(start) => start,
        Err(_) => items.len(),
    };
    if start >= items.len() {
        return finish(Vec::new(), None, total);
    }
    let end = start.saturating_add(limit).min(items.len());
    let next = if end < items.len() {
        Some(end as u64)
    } else {
        None
    };
    finish(items[start..end].to_vec(), next, total)
}

pub(crate) fn find_ignore_case(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    if haystack.is_ascii() && needle.is_ascii() {
        let hay = haystack.as_bytes();
        let ned = needle.as_bytes();
        if ned.len() > hay.len() {
            return None;
        }
        for start in 0..=hay.len() - ned.len() {
            if hay[start..start + ned.len()].eq_ignore_ascii_case(ned) {
                return Some(start);
            }
        }
        return None;
    }
    let needle_lower = needle.to_lowercase();
    for (char_index, (byte_index, _)) in haystack.char_indices().enumerate() {
        if haystack[byte_index..]
            .to_lowercase()
            .starts_with(&needle_lower)
        {
            return Some(char_index);
        }
    }
    None
}
