//! Property-style tests: many small generated or mutated layouts go through
//! both the split and the sequential parse, which must agree.

use super::{check_split, SAMPLE};

/// Deterministic xorshift64* generator, so failures are reproducible.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % n.max(1) as u64).unwrap_or(0)
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items.get(self.below(items.len())).copied().unwrap_or("")
    }
}

/// Game names; the last [`NULL_NAMES`] are null. Plain integers are one key
/// by value: `10`, `0xA`, `+10`, `1_0` and `0o12` are the same key, and so
/// are `1` and `0x1`.
const NAMES: &[&str] = &[
    "A", "B", "C", "D", "E", "F", "G", "'A'", "\"B\"", "Ёлка", ".hack", "1849", "10", "0xA",
    "2048", "+10", "1", "1_0", "\"10\"", "0x1", "0o12", "7 Days", "~", "null",
];

const NULL_NAMES: usize = 2;

/// Lines of the manifest layout; `$` is replaced by a name from [`NAMES`].
const GOOD: &[&str] = &[
    "$: {}",
    "$:",
    "$: null",
    "$: ~",
    "$:\t{}",
    "$: {} # c",
    "$:   # c",
    "  steam:",
    "    id: 1",
    "  alias: $",
    "  files:",
    "    <base>/x: {}",
    "    <base>/y:",
    "      tags: [save]",
    "",
    "# c",
    "  # indented comment",
];

/// Lines that break the layout or the parse (`$` as in [`GOOD`]).
const ODD: &[&str] = &[
    "    id: [oops]",
    "  alias: x",
    "  alias: \"multi",
    "$: line\": {}",
    "  alias: 'open",
    "$: close'",
    "  alias: |",
    "  alias: |+",
    "  alias: >-",
    "$: |",
    "$: |+",
    "    text",
    "  text",
    " ",
    "  ",
    "\t",
    "\t$: {}",
    "#$: 1",
    "---",
    "--- ",
    "--- $: {}",
    "...",
    "... ",
    "null",
    "~",
    "NULL",
    "- x",
    "  - y",
    "? $",
    ": {}",
    "{$: {}}",
    "[$]",
    "&a $: {}",
    "$: &x",
    "$: &x {}",
    "$: *x",
    "<<: {}",
    "  <<: {}",
    "<<: *x",
    "\u{feff}",
    "\u{feff}$: {}",
    "%YAML 1.2",
    "$: !!map",
    "$: !t",
    "$: !!str",
    "$: !",
    "  !t",
    "x: y",
    "$",
    "$ line",
    "$: b: c",
    "$ #: b",
    "$ :",
    "$: {",
    "}",
    "$: [",
    "]",
    "$: {steam: {id: 2},",
    "\0",
    "$: x",
    "$:{}",
    "\u{2028}$: {}",
    "\u{85}",
];

const BREAKS: &[&str] = &["\n", "\n", "\n", "\n", "\n", "\n", "\r\n", "\r\n", "\r"];

fn line(rng: &mut Rng) -> String {
    let template = if rng.below(8) == 0 {
        rng.pick(ODD)
    } else {
        rng.pick(GOOD)
    };
    let name = rng.pick(NAMES);
    template.replace('$', name)
}

/// Random lines with random breaks.
fn random_lines(rng: &mut Rng) -> String {
    let mut text = String::new();
    if rng.below(8) == 0 {
        text.push('\u{feff}');
    }
    for _ in 0..=rng.below(12) {
        text.push_str(&line(rng));
        text.push_str(rng.pick(BREAKS));
    }
    if rng.below(4) == 0 {
        text.pop();
    }
    text
}

/// Entry bodies (`$` as in [`GOOD`]).
const BODIES: &[&str] = &[
    "  steam:\n    id: 1",
    "  alias: $",
    "  files:\n    <base>/x: {}",
    "  files:\n    <base>/y:\n      tags: [save]\n      when:\n        - os: windows",
    "  installDir:\n    $: {}",
    "  # indented comment",
    "  registry:\n    HKEY_CURRENT_USER/Software/$: {}",
    "",
    "# c",
];

/// Entries in the manifest layout, then at most a couple of odd lines or
/// line breaks.
fn manifest_like(rng: &mut Rng) -> String {
    let mut lines: Vec<String> = Vec::new();
    if rng.below(6) == 0 {
        lines.push("---".to_owned());
    }
    // Names that are not null, mostly distinct (`A` and `'A'` are the same).
    let names = &NAMES[..NAMES.len() - NULL_NAMES];
    let first = rng.below(names.len());
    for i in 0..=rng.below(6) {
        let name = names.get((first + i) % names.len()).copied().unwrap_or("A");
        let bodies = rng.below(3);
        // The first 7 lines of `GOOD` are name lines; those with a body
        // below must have no value.
        let entry = if bodies == 0 {
            rng.pick(&GOOD[..7])
        } else {
            rng.pick(&["$:", "$:   # c"])
        };
        lines.push(entry.replace('$', name));
        // Distinct bodies, so that fields do not repeat.
        let body_first = rng.below(BODIES.len());
        for j in 0..bodies {
            let body = BODIES.get((body_first + j) % BODIES.len()).copied();
            let body = body.unwrap_or("").replace('$', rng.pick(names));
            lines.extend(body.split('\n').map(str::to_owned));
        }
    }
    let crlf = rng.below(4) == 0;
    let mut breaks: Vec<&str> = vec![if crlf { "\r\n" } else { "\n" }; lines.len()];
    if rng.below(2) == 0 {
        for _ in 0..=rng.below(2) {
            let at = rng.below(lines.len() + 1);
            match rng.below(3) {
                0 => lines.insert(at.min(lines.len()), line(rng)),
                1 => {
                    if let Some(l) = lines.get_mut(at) {
                        *l = line(rng);
                    }
                }
                _ => {
                    if let Some(b) = breaks.get_mut(at) {
                        *b = rng.pick(BREAKS);
                    }
                }
            }
        }
    }
    breaks.resize(lines.len(), if crlf { "\r\n" } else { "\n" });
    let mut text = String::new();
    if rng.below(8) == 0 {
        text.push('\u{feff}');
    }
    for (line, br) in lines.iter().zip(&breaks) {
        text.push_str(line);
        text.push_str(br);
    }
    text
}

#[test]
fn generated_layouts_parse_the_same_with_and_without_the_split() {
    let mut rng = Rng(0x5eed_0001_d00d_f00d);
    let mut used = 0;
    for i in 0..3000 {
        let text = if i % 3 == 0 {
            random_lines(&mut rng)
        } else {
            manifest_like(&mut rng)
        };
        let parts = 2 + rng.below(3);
        used += usize::from(check_split(&text, parts));
    }
    // The generator must also produce layouts that the split accepts.
    assert!(used > 600, "split used {used} times");
}

/// Whole lines of the sample; the last one keeps no line break.
fn sample_lines() -> Vec<&'static str> {
    SAMPLE.split_inclusive('\n').collect()
}

#[test]
fn mutated_sample_parses_the_same_with_and_without_the_split() {
    let base = sample_lines();
    let mut rng = Rng(0xfeed_beef_0bad_cafe);
    let mut used = 0;
    for _ in 0..300 {
        let mut lines: Vec<String> = base.iter().map(|l| (*l).to_owned()).collect();
        for _ in 0..=rng.below(2) {
            let at = rng.below(lines.len());
            match rng.below(4) {
                0 => lines.insert(at, format!("{}{}", line(&mut rng), rng.pick(BREAKS))),
                1 => {
                    if let Some(l) = lines.get_mut(at) {
                        *l = format!("{}{}", line(&mut rng), rng.pick(BREAKS));
                    }
                }
                2 => {
                    lines.remove(at);
                }
                _ => {
                    if let Some(l) = lines.get_mut(at) {
                        if l.ends_with('\n') {
                            l.pop();
                            l.push_str(rng.pick(BREAKS));
                        }
                    }
                }
            }
        }
        let text = lines.concat();
        let parts = 2 + rng.below(6);
        used += usize::from(check_split(&text, parts));
    }
    assert!(used > 70, "split used {used} times");
}
