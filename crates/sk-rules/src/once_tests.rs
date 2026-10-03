use super::*;

fn rule_id(issue: &ScanIssue) -> &str {
    issue
        .message_args
        .get("rule_id")
        .map(String::as_str)
        .unwrap_or_default()
}

#[test]
fn without_rule_order_the_first_report_wins() {
    let once = OnceIssues::default();
    let first = once.denied("b.rule", RegHive::Hklm, "SOFTWARE\\Secret");
    assert_eq!(first.as_ref().map(rule_id), Some("b.rule"));
    assert_eq!(first.map(|i| i.severity), Some(IssueSeverity::Info));
    // Same key in another case, same pattern: reported already.
    assert!(once
        .denied("a.rule", RegHive::Hklm, "software\\secret")
        .is_none());
    // Another hive is another key.
    assert!(once
        .denied("a.rule", RegHive::Hkcu, "SOFTWARE\\Secret")
        .is_some());
    assert!(once.invalid_regex("b.rule", "(x", "error").is_some());
    assert!(once.invalid_regex("a.rule", "(x", "error").is_none());
    assert!(once.take_ranked().is_empty());
}

#[test]
fn with_rule_order_the_earliest_rule_wins_whatever_the_report_order() {
    let order = ["z.top", "m.mid", "a.low"];
    let reports: [&[&str]; 3] = [
        &["a.low", "m.mid", "z.top"],
        &["z.top", "a.low", "m.mid"],
        &["m.mid", "unknown", "z.top", "a.low"],
    ];
    for reports in reports {
        let once = OnceIssues::default();
        once.rank_by(order);
        for &rule in reports {
            assert!(once
                .denied(rule, RegHive::Hkcu, "Software\\Denied")
                .is_none());
            assert!(once.invalid_regex(rule, "(x", "error").is_none());
        }
        let mut issues = once.take_ranked();
        issues.sort_by(|a, b| a.message_key.cmp(&b.message_key));
        assert_eq!(issues.len(), 2, "{issues:?}");
        assert_eq!(issues[0].message_key, ISSUE_INVALID_REGEX);
        assert_eq!(issues[1].message_key, ISSUE_REGISTRY_ACCESS_DENIED);
        assert!(issues.iter().all(|i| rule_id(i) == "z.top"), "{issues:?}");
        assert!(once.take_ranked().is_empty());
    }
}

#[test]
fn rules_outside_the_order_come_last_by_id() {
    let once = OnceIssues::default();
    once.rank_by(["listed"]);
    once.denied("y.other", RegHive::Hkcu, "Software\\Denied");
    once.denied("x.other", RegHive::Hkcu, "Software\\Denied");
    let issues = once.take_ranked();
    assert_eq!(issues.iter().map(rule_id).collect::<Vec<_>>(), ["x.other"]);

    once.denied("x.other", RegHive::Hkcu, "Software\\Denied");
    once.denied("listed", RegHive::Hkcu, "Software\\Denied");
    let issues = once.take_ranked();
    assert_eq!(issues.iter().map(rule_id).collect::<Vec<_>>(), ["listed"]);
}
