//! Held-out selection regressions: these synthetic cases are independent of the
//! frozen topic-baseline questions and are not measurements of model quality.
use moon::source_learning::{SourcePrepareRequest, SourceProposalInput};
use moon::sources::SourceInput;
use moon::topics::{TopicInput, TopicPrepareRequest, TopicProposalInput};
use moon::{ContextPacket, ContextRequest, MemoryInput, SearchMode, Store};
use serde_json::{Value, json};
use std::collections::BTreeSet;

const SCOPE: &str = "heldout-selection";
const BUDGETS: [usize; 4] = [3_500, 6_000, 8_000, 14_000];

fn text<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field].as_str().unwrap()
}

fn learn_in(store: &mut Store, scope: &str, key: &str, content: &str) -> String {
    let input: SourceInput = serde_json::from_value(json!({
        "schema_version":1,"source_key":key,"kind":"external_document",
        "author":"Synthetic selection reviewer","content":content
    }))
    .unwrap();
    store.source_import(scope, input, None, true).unwrap();
    let prepared = store
        .source_learning_prepare(&SourcePrepareRequest {
            scope: scope.into(),
            key: Some(key.into()),
            max_bytes: 65_536,
            lease_ms: 60_000,
            preview: false,
        })
        .unwrap();
    let proposals: SourceProposalInput =
        serde_json::from_value(json!({"schema_version":1,"claims":[{"quote":content}]})).unwrap();
    let review = store
        .source_learning_submit(
            scope,
            text(&prepared, "job_id"),
            text(&prepared, "lease_token"),
            proposals,
        )
        .unwrap();
    let accepted = store
        .source_learning_accept(
            scope,
            text(&prepared, "job_id"),
            text(&review, "result_hash"),
        )
        .unwrap();
    accepted["claim_ids"][0].as_str().unwrap().to_owned()
}

fn learn(store: &mut Store, key: &str, content: &str) -> String {
    learn_in(store, SCOPE, key, content)
}

fn topic(store: &mut Store, key: &str, title: &str, prose: &str, claims: &[String]) {
    let definition: TopicInput = serde_json::from_value(json!({
        "schema_version":1,"key":key,"title":title,"claim_ids":claims
    }))
    .unwrap();
    store.topic_put(SCOPE, definition, None).unwrap();
    let prepared = store
        .topic_prepare(&TopicPrepareRequest {
            scope: SCOPE.into(),
            key: Some(key.into()),
            max_bytes: 65_536,
            lease_ms: 60_000,
            preview: false,
        })
        .unwrap();
    let proposals: TopicProposalInput = serde_json::from_value(json!({
        "schema_version":1,"review_complete":true,
        "statements":[{"kind":"interpretation","text":prose,"input_ids":claims}]
    }))
    .unwrap();
    let review = store
        .topic_submit(
            SCOPE,
            text(&prepared, "job_id"),
            text(&prepared, "lease_token"),
            proposals,
        )
        .unwrap();
    store
        .topic_accept(
            SCOPE,
            text(&prepared, "job_id"),
            text(&review, "result_hash"),
        )
        .unwrap();
}

fn packet(store: &Store, query: &str, max_chars: usize) -> ContextPacket {
    let packet = store
        .assemble_context(
            &ContextRequest {
                query: query.into(),
                mode: SearchMode::Lexical,
                limit: 8,
                scope: Some(SCOPE.into()),
                max_chars,
                evidence_per_memory: 1,
            },
            None,
        )
        .unwrap();
    assert!(packet.used_chars <= max_chars);
    assert_eq!(packet.used_chars, packet.render_markdown().chars().count());
    assert!(packet.references.len() + packet.memories.len() <= 8);
    for reference in &packet.references {
        assert_eq!(reference.scope, SCOPE);
        if let Some(claim) = &reference.source_claim {
            assert!(!claim.citations.is_empty());
            for citation in &claim.citations {
                assert_eq!(citation.quote, reference.content);
                assert_eq!(
                    citation.end_byte - citation.start_byte,
                    citation.quote.len()
                );
            }
        }
    }
    packet
}

fn source_ids(packet: &ContextPacket) -> BTreeSet<String> {
    packet
        .references
        .iter()
        .filter_map(|reference| reference.source_claim.as_ref())
        .map(|claim| claim.claim_id.clone())
        .collect()
}

#[test]
fn a_long_question_can_retrieve_each_independently_relevant_clause() {
    let temporary = tempfile::tempdir().unwrap();
    let mut store = Store::open(temporary.path().join("state/moon.sqlite"), 64).unwrap();
    let first = learn(
        &mut store,
        "backup",
        "Encrypted backups must be verified before the restore operation begins.",
    );
    let second = learn(
        &mut store,
        "keys",
        "Signing keys rotate only after operator approval; previous keys remain valid during recovery.",
    );
    let third = learn(
        &mut store,
        "restore",
        "An interrupted restore leaves the prior database available until a new image passes verification.",
    );
    let irrelevant = learn(
        &mut store,
        "holiday",
        "The holiday brochure describes encrypted sculptures beside the public gardens.",
    );
    let result = packet(
        &store,
        "Please explain how to verify encrypted backups, rotate signing keys, and recover after an interrupted restore without losing prior records.",
        8_000,
    );
    let ids = source_ids(&result);
    assert!(ids.contains(&first) && ids.contains(&second) && ids.contains(&third));
    assert!(
        !ids.contains(&irrelevant),
        "one weak shared term is not sufficient"
    );
}

#[test]
fn named_topic_supplies_exact_generic_premises_without_global_anchor_relaxation() {
    let temporary = tempfile::tempdir().unwrap();
    let mut store = Store::open(temporary.path().join("state/moon.sqlite"), 64).unwrap();
    let first = learn(
        &mut store,
        "operator",
        "An operator must authorise each production restart; a reviewed plan does not itself grant approval.",
    );
    let second = learn(
        &mut store,
        "readiness",
        "The service becomes ready only after its local integrity check has completed successfully.",
    );
    let other_scope = learn_in(
        &mut store,
        "unrelated-scope",
        "foreign-approval",
        "Harbour production restarts are always approved without a readiness check.",
    );
    topic(
        &mut store,
        "harbour-readiness",
        "Harbour restart readiness",
        "Harbour production restart readiness requires both explicit operator authorisation and a successful integrity check; reviewing a plan grants neither.",
        &[first.clone(), second.clone()],
    );
    let result = packet(
        &store,
        "Does Harbour restart readiness permit an automatic production restart after plan review?",
        8_000,
    );
    let ids = source_ids(&result);
    assert!(ids.contains(&first) && ids.contains(&second));
    assert!(!ids.contains(&other_scope));
    assert!(
        result.memories.is_empty(),
        "derived evidence is not a personal memory"
    );
}

#[test]
fn withdrawn_topic_cannot_expand_generic_premises_and_current_direct_evidence_survives() {
    let temporary = tempfile::tempdir().unwrap();
    let mut store = Store::open(temporary.path().join("state/moon.sqlite"), 64).unwrap();
    let first = learn(
        &mut store,
        "operator",
        "An operator must authorise each production restart; a reviewed plan does not itself grant approval.",
    );
    let second = learn(
        &mut store,
        "readiness",
        "The service becomes ready only after its local integrity check has completed successfully.",
    );
    topic(
        &mut store,
        "harbour-readiness",
        "Harbour restart readiness",
        "Harbour production restart readiness requires both explicit operator authorisation and a successful integrity check; reviewing a plan grants neither.",
        &[first.clone(), second.clone()],
    );
    let query =
        "Does Harbour restart readiness permit an automatic production restart after plan review?";
    let initial = source_ids(&packet(&store, query, 8_000));
    assert!(initial.contains(&first) && initial.contains(&second));
    let fallback = learn(
        &mut store,
        "independent-current",
        "Harbour restart readiness requires a fresh operator request before each production restart.",
    );
    let state = store.source_state(SCOPE, "operator").unwrap();
    store
        .source_set_withdrawn(
            SCOPE,
            "operator",
            text(&state, "revision_id"),
            state["generation"].as_i64().unwrap(),
            true,
            "Synthetic selection withdrawal",
        )
        .unwrap();
    for budget in BUDGETS {
        let result = packet(&store, query, budget);
        let ids = source_ids(&result);
        assert!(ids.contains(&fallback));
        assert!(!ids.contains(&first) && !ids.contains(&second));
        assert!(
            result
                .references
                .iter()
                .all(|reference| reference.topic.is_none())
        );
    }
    assert!(store.health().unwrap().ok);
}

#[test]
fn explicit_named_subjects_reject_competing_subjects_with_shared_keywords() {
    let temporary = tempfile::tempdir().unwrap();
    let mut store = Store::open(temporary.path().join("state/moon.sqlite"), 64).unwrap();
    let intended = learn(
        &mut store,
        "einstein",
        "Einstein described gravity through the curvature of spacetime.",
    );
    let competitor = learn(
        &mut store,
        "newton",
        "Newton described gravity through an attractive force between masses.",
    );
    for budget in BUDGETS {
        let result = packet(&store, "How did Einstein describe gravity?", budget);
        let ids = source_ids(&result);
        assert!(ids.contains(&intended));
        assert!(!ids.contains(&competitor));
    }
}

#[test]
fn mixed_chinese_question_recovers_a_complete_negation_without_accepting_subject_only_noise() {
    let temporary = tempfile::tempdir().unwrap();
    let mut store = Store::open(temporary.path().join("state/moon.sqlite"), 64).unwrap();
    let quote = "Harbour 来源撤回后不能用于召回，但仍保留审计历史。";
    let expected = learn(&mut store, "withdrawal-zh", quote);
    let irrelevant = learn(
        &mut store,
        "menu-zh",
        "Harbour 午餐菜单包括面条、饺子和水果。",
    );
    for budget in BUDGETS {
        let result = packet(
            &store,
            "Harbour 来源撤回之后还能召回吗？审计历史是否保留？",
            budget,
        );
        let ids = source_ids(&result);
        assert!(ids.contains(&expected));
        assert!(!ids.contains(&irrelevant));
        assert!(
            result
                .references
                .iter()
                .any(|reference| reference.content == quote)
        );
    }
}

#[test]
fn ordinary_inflections_and_sentence_initial_question_words_remain_retrievable() {
    let temporary = tempfile::tempdir().unwrap();
    let mut store = Store::open(temporary.path().join("state/moon.sqlite"), 64).unwrap();
    let expected = learn(
        &mut store,
        "inspection",
        "Warehouse inspection records must retain repair details for every damaged crate.",
    );
    let result = packet(
        &store,
        "Explain how damaged crates are repaired during warehouse inspections.",
        6_000,
    );
    assert!(source_ids(&result).contains(&expected));
}

#[test]
fn accepted_claims_are_whole_even_when_a_late_qualification_crosses_chunk_boundaries() {
    let temporary = tempfile::tempdir().unwrap();
    let mut store = Store::open(temporary.path().join("state/moon.sqlite"), 64).unwrap();
    let long_quote = format!(
        "{}The glasshouse irrigation controller must never reopen valves after power loss until an operator verifies the emergency shutoff.",
        "The glasshouse irrigation controller records pressure, temperature and every valve transition for later incident inspection. ".repeat(13)
    );
    assert!(long_quote.chars().count() > 1_400);
    let long = learn(&mut store, "detailed-valves", &long_quote);
    let short_quote =
        "The glasshouse irrigation controller keeps the emergency shutoff closed after power loss.";
    let short = learn(&mut store, "short-valves", short_quote);
    for budget in BUDGETS {
        let result = packet(
            &store,
            "Glasshouse irrigation controller emergency shutoff after power loss",
            budget,
        );
        assert!(source_ids(&result).contains(&short));
        for reference in &result.references {
            if reference
                .source_claim
                .as_ref()
                .is_some_and(|claim| claim.claim_id == long)
            {
                assert_eq!(
                    reference.content, long_quote,
                    "never expose a prefix that loses the final prohibition"
                );
            }
        }
        if budget == 14_000 {
            assert!(
                source_ids(&result).contains(&long),
                "a complete fitting claim remains available"
            );
        }
    }
}

#[test]
fn adding_a_topic_does_not_displace_the_previously_selected_complete_source_evidence() {
    let temporary = tempfile::tempdir().unwrap();
    let mut store = Store::open(temporary.path().join("state/moon.sqlite"), 64).unwrap();
    let claims = [
        "Aster ferry battery inspection requires a signed operator checklist before departure; a scheduled inspection alone is insufficient.",
        "Aster ferry battery inspection records retain the measured charge and temperature; missing readings prohibit departure.",
        "Aster ferry battery inspection approval expires at the next departure and cannot authorise subsequent sailings.",
    ];
    let ids = claims
        .iter()
        .enumerate()
        .map(|(index, quote)| learn(&mut store, &format!("ferry-{index}"), quote))
        .collect::<Vec<_>>();
    let query = "Aster ferry battery inspection approval before departure";
    let before = BUDGETS.map(|budget| source_ids(&packet(&store, query, budget)));
    topic(
        &mut store,
        "aster-battery",
        "Aster ferry battery inspection",
        "Aster ferry departure depends on a signed battery inspection checklist, recorded measurements and approval valid for that departure only.",
        &ids,
    );
    for (index, budget) in BUDGETS.into_iter().enumerate() {
        let result = packet(&store, query, budget);
        assert!(before[index].is_subset(&source_ids(&result)));
        for reference in &result.references {
            if reference.source_claim.is_some() {
                assert!(claims.contains(&reference.content.as_str()));
            }
        }
        assert!(
            result
                .references
                .iter()
                .filter(|reference| reference.topic.is_some())
                .count()
                <= 2
        );
    }
}

#[test]
fn relevant_personal_memory_remains_separate_from_imported_source_assertions() {
    let temporary = tempfile::tempdir().unwrap();
    let mut store = Store::open(temporary.path().join("state/moon.sqlite"), 64).unwrap();
    let memory = store
        .remember(MemoryInput {
            memory_kind: "preference".into(),
            scope: SCOPE.into(),
            title: Some("Nettle workshop preference".into()),
            content: "For Nettle workshop travel I prefer the early ferry.".into(),
            importance: 0.8,
            confidence: 1.0,
            pinned: false,
        })
        .unwrap();
    let source = learn(
        &mut store,
        "workshop-route",
        "Nettle workshop travel includes an early ferry route, but the timetable does not establish anyone's personal preference.",
    );
    let result = packet(&store, "Nettle workshop travel ferry preference", 6_000);
    assert!(
        result
            .memories
            .iter()
            .any(|item| item.document_id == memory.document_id)
    );
    assert!(source_ids(&result).contains(&source));
    assert_eq!(result.memories.len(), 1);
}

#[test]
fn project_name_and_generic_word_distractors_do_not_displace_specific_evidence() {
    let temporary = tempfile::tempdir().unwrap();
    let mut store = Store::open(temporary.path().join("state/moon.sqlite"), 64).unwrap();
    let expected = learn(
        &mut store,
        "archive-validation",
        "Juniper encrypts backup archives, verifies restored manifests, and blocks deployment when verification fails.",
    );
    let distractions = [
        "Juniper deployment notices include the picnic timetable.",
        "Juniper backup stationery is stored in the reception cupboard.",
        "Juniper archives describe the history of the public gardens.",
        "Juniper manifests list the museum's borrowed sculptures.",
    ];
    let distractor_ids = distractions
        .iter()
        .enumerate()
        .map(|(index, quote)| learn(&mut store, &format!("distractor-{index}"), quote))
        .collect::<BTreeSet<_>>();
    for budget in BUDGETS {
        let result = packet(
            &store,
            "How does Juniper encrypt backup archives and verify restored manifests before deployment?",
            budget,
        );
        let ids = source_ids(&result);
        assert!(ids.contains(&expected));
        assert!(ids.is_disjoint(&distractor_ids));
    }
}

#[test]
fn weak_secondary_topic_cannot_expand_an_unrelated_branch_just_because_it_adds_a_query_word() {
    let temporary = tempfile::tempdir().unwrap();
    let mut store = Store::open(temporary.path().join("state/moon.sqlite"), 64).unwrap();
    let expected = learn(
        &mut store,
        "integrity-premise",
        "Every encrypted archive must pass a local integrity check before recovery begins.",
    );
    topic(
        &mut store,
        "juniper-recovery",
        "Juniper archive recovery",
        "Juniper encrypted archive recovery requires successful local integrity verification before recovery begins.",
        std::slice::from_ref(&expected),
    );
    let unrelated = learn(
        &mut store,
        "garden-premise",
        "The public garden holds a sculpture exhibition every winter.",
    );
    topic(
        &mut store,
        "juniper-festivities",
        "Juniper deployment festivities",
        "Juniper deployment festivities include a visit to the public garden sculpture exhibition.",
        std::slice::from_ref(&unrelated),
    );
    for budget in BUDGETS {
        let result = packet(
            &store,
            "How does Juniper verify encrypted archive recovery integrity before deployment?",
            budget,
        );
        let ids = source_ids(&result);
        assert!(ids.contains(&expected));
        assert!(!ids.contains(&unrelated));
        assert!(result.references.iter().all(|reference| {
            reference
                .topic
                .as_ref()
                .is_none_or(|topic| topic.key != "juniper-festivities")
        }));
    }
}

#[test]
fn a_long_chinese_clause_does_not_drown_out_a_short_english_qualification() {
    let temporary = tempfile::tempdir().unwrap();
    let mut store = Store::open(temporary.path().join("state/moon.sqlite"), 64).unwrap();
    let chinese_quote = "Kestrel 权限撤销之后不能继续读取内容，审计日志仍然保持不变。";
    let english_quote = "Kestrel Restore can reactivate earlier approved grants only; it does not approve pending grants automatically.";
    let chinese = learn(&mut store, "kestrel-revocation", chinese_quote);
    let english = learn(&mut store, "kestrel-restoration", english_quote);
    let irrelevant = learn(
        &mut store,
        "kestrel-menu",
        "Kestrel lunch menus change every Thursday and include seasonal fruit.",
    );
    for budget in BUDGETS {
        let result = packet(
            &store,
            "Kestrel 权限撤销之后还能继续读取内容吗？审计日志是否保持不变？Restore approve pending grants automatically?",
            budget,
        );
        let ids = source_ids(&result);
        assert!(ids.contains(&chinese) && ids.contains(&english));
        assert!(!ids.contains(&irrelevant));
        assert!(
            result
                .references
                .iter()
                .any(|reference| reference.content == chinese_quote)
        );
        assert!(
            result
                .references
                .iter()
                .any(|reference| reference.content == english_quote)
        );
    }
}

#[test]
fn only_weak_project_and_generic_word_matches_do_not_become_a_confident_answer() {
    let temporary = tempfile::tempdir().unwrap();
    let mut store = Store::open(temporary.path().join("state/moon.sqlite"), 64).unwrap();
    learn(
        &mut store,
        "wattle-workshops",
        "Wattle documents describe pottery workshops for visitors.",
    );
    for budget in BUDGETS {
        let result = packet(
            &store,
            "How does Wattle validate encrypted backups before restoring archived documents?",
            budget,
        );
        assert!(result.references.is_empty());
        assert!(result.memories.is_empty());
    }
}

#[test]
fn an_oversized_complete_claim_reports_budget_omission_rather_than_missing_evidence() {
    let temporary = tempfile::tempdir().unwrap();
    let mut store = Store::open(temporary.path().join("state/moon.sqlite"), 64).unwrap();
    let quote = format!(
        "{}Never restart the pump until the operator verifies both redundant pressure sensors.",
        "Ibis coolant pressure monitoring records every sensor reading before a pump restart. "
            .repeat(10),
    );
    let expected = learn(&mut store, "coolant-monitoring", &quote);
    let query = "Ibis coolant pressure monitoring before pump restart";
    let small = packet(&store, query, 1_000);
    assert!(small.references.is_empty());
    assert!(small.truncated);
    let markdown = small.render_markdown();
    assert!(markdown.contains("Evidence omitted by packet limits"));
    assert!(!markdown.contains("No relevant retrieved references found"));
    let large = packet(&store, query, 6_000);
    assert!(source_ids(&large).contains(&expected));
    assert!(
        large
            .references
            .iter()
            .any(|reference| reference.content == quote)
    );
}

#[test]
fn a_scope_anchor_cannot_replace_the_explicit_named_subject_of_the_question() {
    let temporary = tempfile::tempdir().unwrap();
    let mut store = Store::open(temporary.path().join("state/moon.sqlite"), 64).unwrap();
    let scope = "project:cedar";
    let query = "Cedar Einstein charts explaining gravity";
    let wrong_subject = learn_in(
        &mut store,
        scope,
        "newton-chart",
        "Newton charts explain gravity through forces and masses.",
    );
    let request = |budget| ContextRequest {
        query: query.into(),
        mode: SearchMode::Lexical,
        limit: 8,
        scope: Some(scope.into()),
        max_chars: budget,
        evidence_per_memory: 1,
    };
    for budget in BUDGETS {
        let result = store.assemble_context(&request(budget), None).unwrap();
        assert!(result.references.is_empty());
        assert!(result.used_chars <= budget);
    }
    let intended = learn_in(
        &mut store,
        scope,
        "einstein-chart",
        "Einstein charts explain gravity through curved spacetime.",
    );
    for budget in BUDGETS {
        let result = store.assemble_context(&request(budget), None).unwrap();
        let ids = source_ids(&result);
        assert!(ids.contains(&intended));
        assert!(!ids.contains(&wrong_subject));
        assert!(
            result
                .references
                .iter()
                .all(|reference| reference.scope == scope)
        );
        assert!(result.used_chars <= budget);
    }
}

#[test]
fn comparisons_keep_each_named_subject_with_shared_topical_evidence() {
    let temporary = tempfile::tempdir().unwrap();
    let mut store = Store::open(temporary.path().join("state/moon.sqlite"), 64).unwrap();
    let einstein = learn(
        &mut store,
        "einstein-comparison",
        "Einstein charts explain gravity through curved spacetime.",
    );
    let newton = learn(
        &mut store,
        "newton-comparison",
        "Newton charts explain gravity through forces and masses.",
    );
    let unrelated = learn(
        &mut store,
        "einstein-music",
        "Einstein donated a violin to the local museum.",
    );
    for budget in BUDGETS {
        let result = packet(&store, "Compare Einstein and Newton gravity charts", budget);
        let ids = source_ids(&result);
        assert!(ids.contains(&einstein) && ids.contains(&newton));
        assert!(
            !ids.contains(&unrelated),
            "a named fragment still needs topical overlap"
        );
    }
}
