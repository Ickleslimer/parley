use parley_health::classifier::{
    classify_codex, classify_grok, classify_parley, extract_grok_error, CodexUsageSample,
    GrokErrorEvidence,
};
use parley_health::schema::{ClosedClass, Source, FREE_USAGE_EXHAUSTED};
use serde_json::json;

fn sample(reached: Option<&str>) -> CodexUsageSample {
    CodexUsageSample {
        used_percent: Some(81.0),
        resets_at: Some("soon".into()),
        plan_type: Some("plus".into()),
        rate_limit_reached_type: reached.map(|value| value.to_string()),
        as_of_ms: 10,
    }
}

#[test]
fn codex_null_or_empty_reached_type_is_usage_sample_not_exhaustion() {
    let classified = classify_codex(&sample(None));
    assert_eq!(classified.class, ClosedClass::UsageSample);
    assert_eq!(classified.source, Source::Codex);

    let classified = classify_codex(&sample(Some("")));
    assert_eq!(classified.class, ClosedClass::UsageSample);

    let classified = classify_codex(&sample(Some("   ")));
    assert_eq!(classified.class, ClosedClass::UsageSample);
}

#[test]
fn codex_non_null_reached_type_is_quota_exhausted() {
    let classified = classify_codex(&sample(Some("primary")));
    assert_eq!(classified.class, ClosedClass::QuotaExhausted);
}

#[test]
fn grok_429_plus_free_usage_code_is_exhaustion() {
    let classified = classify_grok(&GrokErrorEvidence {
        http_status: Some(429),
        provider_code: Some(FREE_USAGE_EXHAUSTED.into()),
        generic_rate_limit: false,
        clipped: false,
        ambiguous: false,
    });
    assert_eq!(classified.class, ClosedClass::QuotaExhausted);
}

#[test]
fn grok_generic_rate_limit_and_unavailable_are_not_exhaustion() {
    for evidence in [
        GrokErrorEvidence {
            http_status: None,
            provider_code: Some("rate_limit".into()),
            generic_rate_limit: true,
            clipped: false,
            ambiguous: false,
        },
        GrokErrorEvidence {
            http_status: Some(503),
            provider_code: None,
            generic_rate_limit: false,
            clipped: false,
            ambiguous: false,
        },
        GrokErrorEvidence {
            http_status: Some(529),
            provider_code: None,
            generic_rate_limit: false,
            clipped: false,
            ambiguous: false,
        },
        GrokErrorEvidence {
            http_status: Some(429),
            provider_code: None,
            generic_rate_limit: false,
            clipped: false,
            ambiguous: false,
        },
    ] {
        let classified = classify_grok(&evidence);
        assert_ne!(classified.class, ClosedClass::QuotaExhausted, "{evidence:?}");
        assert_eq!(classified.class, ClosedClass::CapacityThrottle);
    }
}

#[test]
fn grok_clipped_or_ambiguous_does_not_exhaust_even_with_both_tokens() {
    let clipped = classify_grok(&GrokErrorEvidence {
        http_status: Some(429),
        provider_code: Some(FREE_USAGE_EXHAUSTED.into()),
        generic_rate_limit: false,
        clipped: true,
        ambiguous: false,
    });
    assert_eq!(clipped.class, ClosedClass::TurnError);

    let ambiguous = classify_grok(&GrokErrorEvidence {
        http_status: Some(429),
        provider_code: Some(FREE_USAGE_EXHAUSTED.into()),
        generic_rate_limit: false,
        clipped: false,
        ambiguous: true,
    });
    assert_eq!(ambiguous.class, ClosedClass::TurnError);
}

#[test]
fn extract_uses_structured_fields_and_does_not_require_raw_error_text() {
    let value = json!({
        "hook_event_name": "StopFailure",
        "error": {
            "status": 429,
            "code": FREE_USAGE_EXHAUSTED
        }
    });
    let evidence = extract_grok_error(&value, false);
    assert_eq!(evidence.http_status, Some(429));
    assert_eq!(evidence.provider_code.as_deref(), Some(FREE_USAGE_EXHAUSTED));
    assert!(!evidence.clipped);
    assert_eq!(classify_grok(&evidence).class, ClosedClass::QuotaExhausted);
}

#[test]
fn extract_marks_clipped_stdin_and_unstructured_text_without_tokens_as_non_exhausting() {
    let clipped = extract_grok_error(&json!({"error": {"status": 429, "code": FREE_USAGE_EXHAUSTED}}), true);
    assert!(clipped.clipped);
    assert_eq!(classify_grok(&clipped).class, ClosedClass::TurnError);

    let unstructured = extract_grok_error(&json!({"error": "something went wrong"}), false);
    assert!(unstructured.ambiguous);
    assert_eq!(classify_grok(&unstructured).class, ClosedClass::TurnError);
}

#[test]
fn parley_hints_map_watchdog_and_mcp_and_do_not_invent_quota() {
    assert_eq!(
        classify_parley(false, Some(ClosedClass::WatchdogKilled)).class,
        ClosedClass::WatchdogKilled
    );
    assert_eq!(
        classify_parley(false, Some(ClosedClass::McpStdoutUndelivered)).class,
        ClosedClass::McpStdoutUndelivered
    );
    assert_eq!(
        classify_parley(false, Some(ClosedClass::QuotaExhausted)).class,
        ClosedClass::TurnError
    );
    assert_eq!(
        classify_parley(true, Some(ClosedClass::WatchdogKilled)).class,
        ClosedClass::UsageSample
    );
}
