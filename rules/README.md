# Built-in rules

YAML rule files (`*.yaml`, schema v1, SPEC-04 §4.2) embedded into the binary by
`sk-rules` (`include_dir!`). Only files directly in this folder are loaded, in
alphabetical order. Rule ids must be unique across all files; the
`builtin_rules_valid` test in `sk-rules` fails the build on any problem
(FR-04-07). The rule list is SPEC-04 §4.7.
