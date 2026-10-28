use super::*;
use crate::context::active_index::with_fixture;
use crate::context::root::ResolvedSource;

#[test]
fn newer_native_index_cannot_silently_reuse_an_old_pointer() {
    let (home, _, tip, _) = fixture();
    with_fixture(
        &home.join("state_5.sqlite"),
        &[("source", tip.as_path())],
        || {
            fs::write(home.join("state_6.sqlite"), b"synthetic-new-version").unwrap();
            let error = resolve_codex_rollout(&home.join("sessions"), "source").unwrap_err();
            assert_eq!(error.kind, ErrorKind::Unsupported);
        },
    );
}

#[test]
fn extra_metadata_and_bom_boundaries_are_handled_explicitly() {
    let (home, root, tip, _) = fixture();
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend(fs::read(&root).unwrap());
    fs::write(&root, bytes).unwrap();
    let cut = fs::metadata(&root).unwrap().len();
    fs::write(
        &tip,
        jsonl(&[
            paginated_meta("source", 2, Some(("source", 2, cut))),
            record(3, "child"),
        ]),
    )
    .unwrap();
    resolve_active(&home, &tip, |source| {
        let batch = SharedJsonlReader::open_source(&source)
            .unwrap()
            .read_from(0)
            .unwrap();
        assert_eq!(batch.records[0].start_offset, 3);
        assert_eq!(visible_messages(&batch.records).unwrap().len(), 2);
        OpenOptions::new()
            .append(true)
            .open(&tip)
            .unwrap()
            .write_all(format!("{}\n", session_meta("source")).as_bytes())
            .unwrap();
        assert!(SharedJsonlReader::open_source(&source)
            .unwrap()
            .read_from(0)
            .is_err());
    });
}

#[test]
fn history_identity_pins_shared_child_and_rejects_switch_to_longer_origin() {
    let (home, root, tip, cut) = fixture();
    resolve_active(&home, &tip, |source| {
        assert!(source.identity.retains(&source.parts[0].identity, cut));
        assert!(!source.identity.retains(&source.parts[0].identity, cut + 1));
        let mut changed = source.identity.clone();
        changed.history[1].file_index += 1;
        assert!(changed.retains(&source.identity, cut));
        assert!(!changed.retains(&source.identity, cut + 1));
        let store = store_at(&home.join("journal"));
        let key = key("target", "source");
        commit_seed(&store, &key, &source, 10000, "seed");
        OpenOptions::new()
            .append(true)
            .open(&root)
            .unwrap()
            .write_all(format!("{}\n", record(2, &"different ".repeat(500))).as_bytes())
            .unwrap();
        let original = crate::context::lineage::resolve(
            &fs::canonicalize(home.join("sessions")).unwrap(),
            "source",
            root.clone(),
            std::slice::from_ref(&root),
        )
        .unwrap();
        assert!(store
            .plan_resume_delta(&key, &original, &bounds(10000, 10000, 10000))
            .is_err());
    });
}

#[test]
fn linked_unicode_and_oversized_records_keep_existing_sanitization() {
    let (home, _, tip, cut) = fixture();
    fs::write(
        &tip,
        jsonl(&[
            paginated_meta("source", 2, Some(("source", 2, cut))),
            record(3, "café 😀"),
        ]),
    )
    .unwrap();
    append_oversized_compacted(&tip, MAX_PHYSICAL_RECORD + 25);
    OpenOptions::new()
        .append(true)
        .open(&tip)
        .unwrap()
        .write_all(format!("{}\n", record(5, "after")).as_bytes())
        .unwrap();
    resolve_active(&home, &tip, |source| {
        let batch = SharedJsonlReader::open_source(&source)
            .unwrap()
            .read_from(cut)
            .unwrap();
        let messages = visible_messages(&batch.records).unwrap();
        assert_eq!(
            messages
                .iter()
                .map(|message| message.text.as_str())
                .collect::<Vec<_>>(),
            ["café 😀", "after"]
        );
        assert_eq!(batch.skipped_oversized, 1);
    });
}

#[test]
fn depth_limit_filename_owner_and_source_escape_fail_closed() {
    let (home, _, tip, _) = fixture();
    let mut previous = tip;
    for index in 2..10 {
        let previous_id = if index == 2 {
            "tip".to_string()
        } else {
            format!("child-{}", index - 1)
        };
        previous = child(
            &home,
            "source",
            &format!("child-{index}"),
            (
                &previous_id,
                index * 2,
                fs::metadata(&previous).unwrap().len(),
            ),
            "depth",
        );
    }
    with_fixture(
        &home.join("state_5.sqlite"),
        &[("source", previous.as_path())],
        || {
            assert!(resolve_codex_rollout(&home.join("sessions"), "source").is_err());
        },
    );
    let (other, _, tip, _) = fixture();
    let wrong = tip.with_file_name("rollout-2026-09-21T00-00-00-wrong_tip.jsonl");
    fs::rename(tip, &wrong).unwrap();
    with_fixture(
        &other.join("state_5.sqlite"),
        &[("source", wrong.as_path())],
        || {
            assert!(resolve_codex_rollout(&other.join("sessions"), "source").is_err());
        },
    );
    let (home, _, tip, _) = fixture();
    make_junction(
        &home.join("sessions").join("escaped"),
        &temp_dir("external-root"),
    );
    with_fixture(
        &home.join("state_5.sqlite"),
        &[("source", tip.as_path())],
        || {
            assert!(resolve_codex_rollout(&home.join("sessions"), "source").is_err());
        },
    );
}

fn paginated_meta(session: &str, ordinal: u64, base: Option<(&str, u64, u64)>) -> String {
    let base = base.map_or(String::new(), |(id, ordinal, offset)| format!(
        r#", "history_base":{{"thread_id":"{id}","end_ordinal_exclusive":{ordinal},"end_byte_offset":{offset}}}"#));
    format!(
        r#"{{"ordinal":{ordinal},"type":"session_meta","payload":{{"id":"{session}","history_mode":"paginated"{base}}}}}"#
    )
}

fn record(ordinal: u64, text: &str) -> String {
    format!(
        r#"{{"ordinal":{ordinal},"type":"response_item","payload":{{"type":"message","role":"user","id":"message-{ordinal}","turn_id":"turn-{ordinal}","content":[{{"type":"input_text","text":"{text}"}}]}}}}"#
    )
}

fn child(home: &Path, session: &str, id: &str, base: (&str, u64, u64), text: &str) -> PathBuf {
    write_rollout(
        home,
        &format!("{session}_{id}"),
        &jsonl(&[
            paginated_meta(session, base.1, Some(base)),
            record(base.1 + 1, text),
        ]),
    )
}

fn fixture() -> (PathBuf, PathBuf, PathBuf, u64) {
    let home = temp_dir("paginated");
    let prefix = jsonl(&[paginated_meta("source", 0, None), record(1, "seed")]);
    let root = write_rollout(&home, "source", &prefix);
    let cut = prefix.len() as u64;
    let tip = child(&home, "source", "tip", ("source", 2, cut), "child");
    (home, root, tip, cut)
}

fn resolve_active(home: &Path, tip: &Path, check: impl FnOnce(ResolvedSource)) {
    with_fixture(&home.join("state_5.sqlite"), &[("source", tip)], || {
        check(resolve(home, "source"))
    });
}

#[test]
fn active_index_selects_exact_branch_not_a_unique_or_newest_child() {
    let (home, root, tip, cut) = fixture();
    child(
        &home,
        "source",
        "inactive",
        ("source", 2, cut),
        "wrong branch",
    );
    resolve_active(&home, &tip, |source| {
        assert_eq!(source.parts.len(), 2);
        assert_eq!(source.path, fs::canonicalize(&root).unwrap());
        let mut reader = SharedJsonlReader::open_source(&source).unwrap();
        let batch = reader.read_from(0).unwrap();
        assert_eq!(visible_messages(&batch.records).unwrap().len(), 2);
        assert!(!batch
            .records
            .iter()
            .any(|row| row.text.contains("wrong branch")));
    });
    let (home, root, _, _) = fixture();
    resolve_active(&home, &root, |source| assert_eq!(source.parts.len(), 1));
}

#[test]
fn missing_index_never_guesses_a_paginated_tip() {
    let (home, _, _, _) = fixture();
    let error = resolve_codex_rollout(&home.join("sessions"), "source").unwrap_err();
    assert_eq!(error.kind, ErrorKind::Duplicate);
}

#[test]
fn indexed_missing_wrong_session_escape_and_duplicate_rows_fail_closed() {
    for case in ["missing", "foreign", "outside", "duplicate", "no-row"] {
        let (home, _, tip, _) = fixture();
        let selected = match case {
            "missing" => home.join("sessions").join("missing.jsonl"),
            "foreign" => write_rollout(&home, "foreign", &jsonl(&[session_meta("foreign")])),
            "outside" => write_rollout(
                &temp_dir("outside"),
                "source",
                &jsonl(&[session_meta("source")]),
            ),
            _ => tip,
        };
        let rows = match case {
            "duplicate" => vec![
                ("source", selected.as_path()),
                ("source", selected.as_path()),
            ],
            "no-row" => Vec::new(),
            _ => vec![("source", selected.as_path())],
        };
        with_fixture(&home.join("state_5.sqlite"), &rows, || {
            assert!(
                resolve_codex_rollout(&home.join("sessions"), "source").is_err(),
                "{case}"
            );
        });
    }
}

#[test]
fn corrupt_index_is_not_a_filesystem_fallback() {
    let (home, _, _, _) = fixture();
    fs::write(home.join("state_5.sqlite"), b"not sqlite").unwrap();
    assert!(resolve_codex_rollout(&home.join("sessions"), "source").is_err());
}

#[test]
fn old_cursor_keeps_unread_prefix_then_child_and_excludes_abandoned_tail() {
    let (home, root, tip, _) = fixture();
    let store = store_at(&home.join("journal"));
    let key = key("target", "source");
    let original = ResolvedSource {
        session_id: "source".to_string(),
        path: root.clone(),
        identity: SharedJsonlReader::open(&root).unwrap().identity().clone(),
        parts: Vec::new(),
        selection: None,
    };
    let initial = commit_seed(&store, &key, &original, 10000, "seed");
    let unread = format!("{}\n", record(2, "retained gap"));
    OpenOptions::new()
        .append(true)
        .open(&root)
        .unwrap()
        .write_all(unread.as_bytes())
        .unwrap();
    let cut = fs::metadata(&root).unwrap().len();
    fs::write(
        &tip,
        jsonl(&[
            paginated_meta("source", 3, Some(("source", 3, cut))),
            record(4, "child"),
        ]),
    )
    .unwrap();
    OpenOptions::new()
        .append(true)
        .open(&root)
        .unwrap()
        .write_all(format!("{}\n", record(3, "abandoned")).as_bytes())
        .unwrap();
    let journal_path = home.join("journal/target/codex/source/journal.jsonl");
    let prior = fs::read(&journal_path).unwrap();
    resolve_active(&home, &tip, |source| {
        let plan = store
            .plan_resume_delta(&key, &source, &bounds(10000, 10000, 10000))
            .unwrap();
        assert_eq!(plan.start_offset, initial.end_offset);
        assert!(plan.text.contains("retained gap"));
        assert!(plan.text.contains("child"));
        assert!(!plan.text.contains("abandoned"));
        store.append_in_flight(&key, &plan, "delta").unwrap();
        store.commit(&key, &plan, "delta").unwrap();
        assert!(fs::read(&journal_path).unwrap().starts_with(&prior));
        let restarted = store_at(&home.join("journal"));
        let next = restarted
            .plan_resume_delta(&key, &source, &bounds(10000, 10000, 10000))
            .unwrap();
        assert!(next.text.is_empty());
        assert_eq!(next.start_offset, plan.end_offset);
        assert!(fs::read_to_string(&journal_path)
            .unwrap()
            .contains("\"schema_version\":2"));
    });
}

#[test]
fn prefix_only_fingerprint_is_unchanged_and_cut_reads_first_child_payload() {
    let (home, root, tip, cut) = fixture();
    let original = SharedJsonlReader::open(&root)
        .unwrap()
        .read_until(0, cut)
        .unwrap();
    resolve_active(&home, &tip, |source| {
        let mut reader = SharedJsonlReader::open_source(&source).unwrap();
        let prefix = reader.read_until(0, cut).unwrap();
        assert_eq!(original.fingerprint, prefix.fingerprint);
        let child = reader.read_from(cut).unwrap();
        assert_eq!(child.records.len(), 1);
        assert_eq!(child.records[0].start_offset, cut);
        assert!(child.records[0].text.contains("child"));
    });
}

#[test]
fn malformed_lineages_fail_before_streaming() {
    for case in [
        "missing",
        "ambiguous",
        "short",
        "mid-record",
        "ordinal",
        "self",
        "header",
        "cycle",
    ] {
        let (home, root, tip, cut) = fixture();
        let base = match case {
            "missing" => ("absent", 2, cut),
            "short" => ("source", 2, cut + 1),
            "mid-record" => ("source", 2, cut - 1),
            "ordinal" => ("source", 9, cut),
            "self" => ("tip", 2, cut),
            _ => ("source", 2, cut),
        };
        fs::write(
            &tip,
            jsonl(&[
                paginated_meta(
                    if case == "header" { "wrong" } else { "source" },
                    base.1,
                    Some(base),
                ),
                record(base.1 + 1, "child"),
            ]),
        )
        .unwrap();
        if case == "ambiguous" {
            let other = home.join("sessions/other");
            fs::create_dir(&other).unwrap();
            fs::copy(&root, other.join(root.file_name().unwrap())).unwrap();
        }
        if case == "cycle" {
            fs::write(
                &root,
                jsonl(&[
                    paginated_meta("source", 2, Some(("tip", 2, cut))),
                    record(3, "cycle"),
                ]),
            )
            .unwrap();
        }
        with_fixture(&home.join("state_5.sqlite"), &[("source", &tip)], || {
            assert!(
                resolve_codex_rollout(&home.join("sessions"), "source").is_err(),
                "{case}"
            );
        });
    }
}

#[test]
fn child_replacement_truncation_and_header_change_are_detected() {
    for case in [
        "replace-before-open",
        "replace-during-read",
        "truncate",
        "header",
    ] {
        let (home, _, tip, cut) = fixture();
        resolve_active(&home, &tip, |source| {
            if case == "replace-before-open" {
                fs::rename(&tip, tip.with_extension("preserved")).unwrap();
                fs::write(
                    &tip,
                    jsonl(&[
                        paginated_meta("source", 2, Some(("source", 2, cut))),
                        record(3, "child"),
                    ]),
                )
                .unwrap();
                assert!(SharedJsonlReader::open_source(&source).is_err());
                return;
            }
            let mut reader = SharedJsonlReader::open_source(&source).unwrap();
            let result = reader.scan_from(0, |_| {
                match case {
                    "replace-during-read"
                        if tip.exists() && !tip.with_extension("preserved").exists() =>
                    {
                        fs::rename(&tip, tip.with_extension("preserved")).unwrap();
                        fs::copy(tip.with_extension("preserved"), &tip).unwrap();
                    }
                    "truncate" => {
                        OpenOptions::new()
                            .write(true)
                            .open(&tip)
                            .unwrap()
                            .set_len(0)
                            .unwrap();
                    }
                    "header" => {
                        let body = fs::read_to_string(&tip)
                            .unwrap()
                            .replace("paginated", "PAGINATED");
                        fs::write(&tip, body).unwrap();
                    }
                    _ => {}
                }
                Ok(())
            });
            assert!(result.is_err(), "{case}");
        });
    }
}

#[test]
fn nested_lineage_and_incomplete_tip_append_keep_order() {
    let (home, _, tip, _) = fixture();
    let tip_end = fs::metadata(&tip).unwrap().len();
    let newest = child(&home, "source", "newest", ("tip", 4, tip_end), "newest");
    OpenOptions::new()
        .append(true)
        .open(&newest)
        .unwrap()
        .write_all(b"{\"ordinal\":6")
        .unwrap();
    resolve_active(&home, &newest, |source| {
        assert_eq!(source.parts.len(), 3);
        let batch = SharedJsonlReader::open_source(&source)
            .unwrap()
            .read_from(0)
            .unwrap();
        let text = visible_messages(&batch.records)
            .unwrap()
            .iter()
            .map(|message| message.text.clone())
            .collect::<Vec<_>>();
        assert_eq!(text, ["seed", "child", "newest"]);
        assert_eq!(batch.incomplete_tail, b"{\"ordinal\":6");
    });
}

#[test]
fn committed_child_replacement_or_revert_behind_cursor_never_jumps() {
    for replace in [true, false] {
        let (home, root, tip, cut) = fixture();
        resolve_active(&home, &tip, |source| {
            let store = store_at(&home.join("journal"));
            let key = key("target", "source");
            commit_seed(&store, &key, &source, 10000, "seed");
            let before = store.inspect(&key).unwrap();
            if replace {
                fs::rename(&tip, tip.with_extension("preserved")).unwrap();
                fs::write(
                    &tip,
                    jsonl(&[
                        paginated_meta("source", 2, Some(("source", 2, cut))),
                        record(3, "replacement"),
                    ]),
                )
                .unwrap();
            } else {
                fs::write(
                    &tip,
                    jsonl(&[
                        paginated_meta(
                            "source",
                            1,
                            Some((
                                "source",
                                1,
                                paginated_meta("source", 0, None).len() as u64 + 1,
                            )),
                        ),
                        record(2, "reverted"),
                    ]),
                )
                .unwrap();
            }
            let fresh = resolve(&home, "source");
            assert!(store
                .plan_resume_delta(&key, &fresh, &bounds(10000, 10000, 10000))
                .is_err());
            assert_eq!(store.inspect(&key).unwrap(), before);
            assert!(root.exists());
        });
    }
}
