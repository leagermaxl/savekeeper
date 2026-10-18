use sk_core::model::FolderSummary;
use sk_core::template::PathTemplate;

use super::*;

fn info() -> ClassifierInfo {
    ClassifierInfo {
        provider: "mock",
        model: "m".to_owned(),
        is_local: true,
        max_batch: 8,
        max_concurrency: 1,
    }
}

fn item(id: &str, name: &str, sample_len: usize) -> ClassifyItem {
    ClassifyItem {
        id: id.to_owned(),
        summary: FolderSummary {
            path: PathTemplate::parse(&format!(r"{{APPDATA}}\{name}")).unwrap(),
            total_bytes: 1,
            file_count: 1,
            dir_count: 0,
            max_depth: 0,
            newest_mtime: None,
            oldest_mtime: None,
            ext_histogram: Vec::new(),
            // Words, so no sample looks like a token.
            sample_names: (0..sample_len / 10)
                .map(|i| format!("file {i:04}"))
                .collect(),
            top_children: Vec::new(),
            markers: Vec::new(),
            truncated: false,
        },
        peek: Vec::new(),
        hint: None,
    }
}

#[test]
fn groups_by_cache_key_in_first_occurrence_order() {
    let groups = group(
        vec![
            item("b", "B", 0),
            item("a", "A", 0),
            item("b2", "B", 0),
            item("b", "B", 0),
        ],
        &info(),
    );
    let ids: Vec<Vec<&str>> = groups
        .iter()
        .map(|g| g.ids.iter().map(String::as_str).collect())
        .collect();
    assert_eq!(ids, [vec!["b", "b2"], vec!["a"]]);
    assert_eq!(groups[0].item.id, "b");
}

#[test]
fn batches_respect_count_and_input_size() {
    let small: Vec<Group> = group(
        (0..5)
            .map(|i| item(&i.to_string(), &format!("S{i}"), 0))
            .collect(),
        &info(),
    );
    let sizes: Vec<usize> = batches(small, 2).iter().map(Vec::len).collect();
    assert_eq!(sizes, [2, 2, 1]);

    // ~4 000 characters each (the summary limit): three fit into 12 000 only as two.
    let big = group(
        (0..5)
            .map(|i| item(&i.to_string(), &format!("B{i}"), 5_000))
            .collect(),
        &info(),
    );
    assert!(big.iter().all(|g| g.chars > 3_500 && g.chars < 4_200));
    let out = batches(big, 8);
    assert!(out
        .iter()
        .all(|b| b.iter().map(|g| g.chars).sum::<usize>() <= MAX_BATCH_INPUT_CHARS));
    assert!(out.len() >= 2);
    assert_eq!(out.iter().map(Vec::len).sum::<usize>(), 5);
}

#[test]
fn an_oversized_item_goes_alone() {
    let mut huge = item("h", "H", 0);
    // `path` is never cut, so the item stays larger than the batch limit.
    huge.summary.path =
        PathTemplate::parse(&format!(r"{{APPDATA}}\{}", "dir ".repeat(4_000).trim())).unwrap();
    let groups = group(vec![item("a", "A", 0), huge, item("b", "B", 0)], &info());
    assert!(groups[1].chars > MAX_BATCH_INPUT_CHARS);
    let sizes: Vec<usize> = batches(groups, 8).iter().map(Vec::len).collect();
    assert_eq!(sizes, [1, 1, 1]);
}

#[test]
fn sort_is_by_path() {
    let mut groups = group(
        vec![item("c", "C", 0), item("a", "A", 0), item("b", "B", 0)],
        &info(),
    );
    sort(&mut groups);
    let ids: Vec<&str> = groups.iter().map(|g| g.item.id.as_str()).collect();
    assert_eq!(ids, ["a", "b", "c"]);
}
