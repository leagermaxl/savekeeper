use super::*;

const SAMPLE: &str = include_str!("../../../../fixtures/samples/ludusavi/manifest-sample.yaml");
const MINI: &str = include_str!("../../../../fixtures/samples/ludusavi/manifest-mini.yaml");

type Outcome = Result<HashMap<String, GameEntry>, String>;

fn sequential(text: &str) -> Outcome {
    parse_in_parts(text.as_bytes(), 1).map_err(|e| e.to_string())
}

/// The split path, run on `parts` chunks, never disagrees with a sequential
/// parse: when it gives a result, the sequential parse gives the same one.
/// Returns whether the split result was used.
fn check_split(text: &str, parts: usize) -> bool {
    let seq = sequential(text);
    let split = parse_split(text, parts);
    if let Some(games) = &split {
        assert_eq!(seq.as_ref(), Ok(games), "parts = {parts}\n{text:?}");
    }
    let par = parse_in_parts(text.as_bytes(), parts).map_err(|e| e.to_string());
    assert_eq!(seq, par, "parts = {parts}\n{text:?}");
    split.is_some()
}

/// [`check_split`] for 2..=4 parts; whether any of them used the split.
fn assert_same(text: &str) -> bool {
    // Every number of parts is checked, not only up to the first split.
    let used: Vec<bool> = (2..=4).map(|parts| check_split(text, parts)).collect();
    used.contains(&true)
}

#[test]
fn sequential_budget_grows_with_input_and_keeps_defaults_for_small_input() {
    let small = sequential_budget(10);
    let default = Budget::default();
    assert_eq!(small.max_events, default.max_events);
    assert_eq!(small.max_nodes, default.max_nodes);
    assert_eq!(small.max_aliases, default.max_aliases);

    let big = sequential_budget(40 << 20);
    assert!(big.max_events >= 80 << 20);
    assert!(big.max_nodes >= 80 << 20);
    assert!(big.max_total_scalar_bytes >= 40 << 20);
    assert_eq!(big.max_aliases, default.max_aliases);
    assert_eq!(big.max_anchors, default.max_anchors);
}

#[test]
fn chunk_budgets_sum_to_at_most_the_sequential_budget() {
    for total in [10, 1000, 3 << 20, 40 << 20, 41_943_041] {
        for parts in [2, 3, 7, 16] {
            let lens: Vec<usize> = (0..parts)
                .map(|k| total / parts + usize::from(k < total % parts))
                .collect();
            assert_eq!(lens.iter().sum::<usize>(), total);
            let budgets: Vec<Budget> = lens
                .iter()
                .map(|&len| chunk_options(len, total).budget.unwrap_or_default())
                .collect();
            let seq = sequential_budget(total);
            let sum = |f: fn(&Budget) -> usize| budgets.iter().map(f).sum::<usize>();
            assert!(sum(|b| b.max_events) <= seq.max_events);
            assert!(sum(|b| b.max_nodes) <= seq.max_nodes);
            assert!(sum(|b| b.max_total_scalar_bytes) <= seq.max_total_scalar_bytes);
            assert!(sum(|b| b.max_total_comment_bytes) <= seq.max_total_comment_bytes);
            for b in &budgets {
                assert_eq!((b.max_aliases, b.max_anchors, b.max_merge_keys), (0, 0, 0));
                assert_eq!(b.max_recorded_anchor_events, 0);
                assert_eq!(b.max_recorded_anchor_bytes, 0);
                assert_eq!(b.max_documents, 1);
                assert_eq!(b.max_depth, seq.max_depth);
            }
        }
    }
}

#[test]
fn anchors_aliases_and_merge_keys_fall_back_to_sequential() {
    for text in [
        // Anchor and alias in the same chunk.
        "A: &x\n  steam:\n    id: 7\nB: *x\nC: {}\nD: {}\n",
        // Alias to an anchor in another chunk.
        "A: &x\n  steam:\n    id: 7\nB: {}\nC: *x\n",
        // An anchor alone.
        "A: &x {}\nB: {}\nC: {}\n",
        // Merge keys.
        "A:\n  <<: {steam: {id: 3}}\nB: {}\nC: {}\n",
        "<<: {}\nB: {}\nC: {}\n",
    ] {
        for parts in 2..=4 {
            assert!(parse_split(text, parts).is_none(), "{parts} {text:?}");
        }
        assert_same(text);
    }
    let text = "A: &x\n  steam:\n    id: 7\nB: {}\nC: *x\n";
    let games = parse_in_parts(text.as_bytes(), 3).unwrap_or_default();
    assert_eq!(games["C"], games["A"]);
    assert_eq!(games["C"].steam.map(|s| s.id), Some(7));
}

#[test]
fn split_covers_text_and_cuts_at_entries() {
    let starts = entry_starts(SAMPLE).unwrap_or_default();
    assert_eq!(starts.len(), 34);
    for parts in 2..=12 {
        let chunks = split(SAMPLE, parts).unwrap_or_default();
        assert!(chunks.len() > 1 && chunks.len() <= parts, "{parts}");
        let texts: Vec<&str> = chunks.iter().map(|c| c.text).collect();
        assert_eq!(texts.concat(), SAMPLE);
        assert_eq!(chunks.iter().map(|c| c.entries).sum::<usize>(), 34);
        for chunk in &chunks[1..] {
            let first = chunk.text.lines().next().unwrap_or_default();
            assert!(is_entry_start(first), "{first:?}");
        }
        assert!(parse_split(SAMPLE, parts).is_some(), "{parts}");
    }
    assert!(split(SAMPLE, 8).unwrap_or_default().len() > 4);
    // Nothing to cut at.
    assert_eq!(split("A:\n  alias: B\n", 4), None);
}

#[test]
fn fixtures_parse_the_same_with_the_split() {
    for text in [SAMPLE, MINI] {
        for parts in 2..=12 {
            assert!(check_split(text, parts), "{parts}");
        }
    }
    let games = parse_in_parts(SAMPLE.as_bytes(), 6).unwrap_or_default();
    assert_eq!(games.len(), 34);
}

#[test]
fn entry_start_lines() {
    for line in [
        "A:",
        "A: {}",
        "A:\t{}",
        "Half-Life 2: Episode One:",
        "A: b: c",
        "A [x]: {}",
        ".hack//G.U.: {}",
        "(The) Game:",
        "~:",
        "Ёлка: {}",
        "Sekiro™ Shadows Die Twice:",
        "'1849':",
        "'it''s': {}",
        "\"say \\\"hi\\\"\": {}",
        "\"a: b\":",
        "A :",
    ] {
        assert!(is_entry_start(line), "{line:?}");
    }
    for line in [
        "",
        " A: {}",
        "\tA: {}",
        "# A: {}",
        "A",
        "A:b",
        "A #: b",
        "---",
        "--- A: {}",
        "...",
        "...: {}",
        "- A: {}",
        "? A",
        ": A",
        "{A: {}}",
        "[A]: {}",
        "&a A: {}",
        "*a: {}",
        "!t A: {}",
        "|: {}",
        ">: {}",
        "%YAML 1.2",
        "@a: {}",
        "`a: {}",
        "\u{feff}A: {}",
        "\u{a0}A: {}",
        "\u{2028}A: {}",
        "'open: {}",
        "\"open: {}",
        "'q' : {}",
        "\"q\"x: {}",
    ] {
        assert!(!is_entry_start(line), "{line:?}");
    }
}

#[test]
fn whitelisted_layouts_are_split() {
    for text in [
        "A: {}\nB: {}\nC: {}\n",
        "A: {}\r\nB: {}\r\nC: {}\r\n",
        "\u{feff}A: {}\nB: {}\nC: {}\n",
        "# head\n\n---\n# more\nA: {}\nB: {}\nC: {}",
        "A:\n  steam:\n    id: 1\n# c\n\nB:\n  files:\n    <base>/x: {}\n  \nC: null\nD: ~\n",
        "'1': {}\n\"2\": {}\n.3: {}\n",
    ] {
        assert!(entry_starts(text).is_some(), "{text:?}");
        assert!(assert_same(text), "{text:?}");
    }
}

#[test]
fn layouts_outside_the_whitelist_are_not_split() {
    for text in [
        // Review round 2: a null scalar as a chunk or as the root.
        "A: {}\nB: {}\nnull\n",
        "A: {}\nB: {}\n~\n",
        "A: {}\nB: {}\nNULL\n",
        "~\n#pad\nA: {}\n",
        "null\nA: {}\nB: {}\n",
        // Indented or flow root.
        "  A: {}\n  B: {}\nC: {}\n",
        "{A: {}}\nB: {}\n",
        "[A]\nB: {}\n",
        // Bare `\r` line breaks, NUL.
        "A: {}\r...\r\nB: {}\n",
        "A: {}\rB: {}\nC: {}\n",
        "A: {}\nB: {}\nC: {}\r",
        "A: {}\nB: {}\0\nC: {}\n",
        // Document markers, directives, byte order marks.
        "A: {}\nB: {}\n...\nC: {}\n",
        "A: {}\nB: {}\n---\nC: {}\n",
        "A: {}\nB: {}\nC: {}\n...\n",
        "A: {}\nB: {}\n...\n]]]\nC: {}\n",
        "---\n---\nA: {}\nB: {}\n",
        "--- # c\nA: {}\nB: {}\n",
        "%YAML 1.2\n---\nA: {}\nB: {}\nC: {}\n",
        "A: {}\n\u{feff}B: {}\nC: {}\n",
        "A: {}\nB: {}\n\u{feff}C: {}\n",
        // Other column-0 lines.
        "A: {}\n- x\nB: {}\n",
        "A:\n- x\nB: {}\n",
        "A: {}\n? B\n: {}\nC: {}\n",
        "A:\n  alias: first\nB line\nC: {}\n",
        "A: {}\n\tB: {}\nC: {}\n",
        "A: {}\n&a B: {}\nC: *a\n",
        // No name line at all.
        "",
        "# only a comment\n",
        "- A\n- B\n",
    ] {
        assert_eq!(entry_starts(text), None, "{text:?}");
        assert!(!assert_same(text), "{text:?}");
    }
}

#[test]
fn chunk_failures_fall_back_to_sequential() {
    for text in [
        // A cut inside a multi-line quoted scalar.
        "A:\n  alias: \"first\nB: line\"\nC: {}\n",
        "A:\n  alias: 'first\nB: line'\nC: {}\n",
        // Errors in one chunk, with whole-file positions.
        "A: {}\nB: {}\nC: {}\nD:\n  steam:\n    id: [oops]\n",
        // A game name repeated within a chunk or across chunks.
        "A:\n  steam:\n    id: 1\nB: {}\nA:\n  steam:\n    id: 2\n",
        "A: {}\nA: {}\nB: {}\nC: {}\n",
        "A: {}\n'A': {}\nB: {}\n",
        "A: {}\nB: {}\n\"A\": {}\n",
        // Plain integer names are one key by value, whatever the spelling.
        "10: {}\nB: {}\n0xA: {}\n",
        "1_0: {}\nB: {}\n10: {}\n",
        "+1: {}\nB: {}\n1: {}\n",
        "1: {}\nB: {}\n0x1: {}\n",
        "10: {}\nB: {}\n+10: {}\n",
        "10: {}\nB: {}\n0o12: {}\n",
        "0b10: {}\nB: {}\n2: {}\n",
        "10: {}\nB: {}\n10\u{3000}: {}\n",
    ] {
        for parts in 2..=4 {
            assert!(parse_split(text, parts).is_none(), "{parts} {text:?}");
        }
        assert_same(text);
        assert!(sequential(text).is_err(), "{text:?}");
    }
    // A flow collection may go on at column 0: the first chunk is then
    // unterminated, and the sequential parse sees one game with a `B` field.
    let text = "A: {steam: {id: 1},\nB: 2}\nC: {}\n";
    assert!(parse_split(text, 2).is_none());
    assert_same(text);
    let games = sequential(text).unwrap_or_default();
    assert_eq!(games.len(), 2);
    assert_eq!(games["A"].steam.map(|s| s.id), Some(1));
}

#[test]
fn names_that_look_like_scalars_keep_the_split_when_distinct() {
    for text in [
        "2048: {}\nB: {}\n7 Days to Die: {}\n\"1979 Revolution: Black Friday\": {}\n",
        "10: {}\nB: {}\n0xB: {}\n1.0: {}\n",
        "true: {}\nB: {}\nTrue: {}\n",
    ] {
        assert!(assert_same(text), "{text:?}");
    }
    // Different keys for the parser, but the same name: no split.
    for text in ["10: {}\nB: {}\n\"10\": {}\n", "1.0: {}\nB: {}\n'1.0': {}\n"] {
        assert!(!assert_same(text), "{text:?}");
    }
    for name in [
        "2048", "+1", "0xA", "1_0", "7 Days", ".hack", "~", "null", "No", "y",
    ] {
        assert!(may_be_non_string(name), "{name:?}");
    }
    for name in ["A", "Ёлка", "Noita", "yes2", "x10"] {
        assert!(!may_be_non_string(name), "{name:?}");
    }
    assert_eq!(plain_name("10 : {}\nB: {}\n"), Some("10 "));
    assert_eq!(plain_name("+1:\r\n"), Some("+1"));
    assert_eq!(plain_name("\"10\": {}\n"), None);
    assert_eq!(plain_name("'10': {}\n"), None);
}

#[test]
fn invalid_utf8_is_an_error_in_any_number_of_parts() {
    for parts in 1..=3 {
        let r = parse_in_parts(b"A: {}\nB: \xff\nC: {}\n", parts);
        assert!(matches!(r, Err(GamesError::ManifestParse(_))), "{r:?}");
    }
}

#[path = "parse_prop_tests.rs"]
mod prop;
