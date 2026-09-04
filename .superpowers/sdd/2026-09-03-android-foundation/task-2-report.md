# Task 2 report — enforce CLI recommendations

## Implementation

- Added nullable, backward-compatible `recommendation` and `recommended_choice` fields to
  `client::Item`, and optional serialization of both fields on `client::NewItem`.
- Added `--recommendation <TEXT>` and `--recommended-choice <CHOICE>` to `lam push` and passed
  them through `PushArgs` to the Worker payload.
- Added one pre-config/pre-network `validate_push` path. It requires a non-empty recommendation
  for every non-checklist request; requires an exact member `--recommended-choice` for choices;
  rejects recommendation flags on checklists and recommended choices without choices; retains the
  existing choice/check exclusivity and link validation.
- Mirrored the Worker content limits locally: title/choice/check at 200 Unicode code points,
  recommendation at 2,000 Unicode code points, body at 65,536 UTF-8 bytes, at most three choices,
  and at most 50 checks.
- Updated the agent skill (and therefore `lam --llm`) and README command syntax. Decision examples
  now include an action/rationale recommendation, and the choice example includes both flags.

## Files changed

- `cli/src/client.rs`
- `cli/src/commands.rs`
- `cli/src/main.rs`
- `cli/tests/cli.rs`
- `skill/lam/SKILL.md`
- `README.md`
- `cli/src/tui/mod.rs` — scope exception authorized by the task owner: added `None` values only to
  the existing test fixture after the `Item` model gained two mandatory Rust struct fields; no TUI
  behavior changed.

## TDD evidence

Before writing production code, added the focused integration cases for the recommendation
contract and ran:

```text
$ cd cli && cargo test --test cli push_
running 9 tests
... 6 failed; 3 passed
... unexpected argument '--recommendation' found
... --recommendation is required for every non-checklist request [assertion failure]
```

This RED result proved the new flags were absent and local validation had not yet intercepted the
invalid requests.

After the minimal implementation, ran:

```text
$ cd cli && cargo test --test cli push_ && cargo test --test cli push_without_any_name_source_fails_with_guidance
running 9 tests
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 9 filtered out
running 1 test
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 17 filtered out
```

The tests exercise real CLI processes and a real local Wiremock server where a valid payload is
needed. Invalid input uses a missing config path to demonstrate validation occurs before config
loading or network access. Cases cover missing recommendation(s), exact and non-member choices,
checklist compatibility, disallowed flags, every approved content limit, and serialized payload
fields.

## Final verification

```text
$ cd cli && cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test
clippy: Finished `dev` profile ...
unit tests: 36 passed; 0 failed
integration tests: 18 passed; 0 failed
```

`git diff --check` also passed.

## Self-review

- Validation runs as the first operation in `commands::push`, before name resolution, duration
  parsing, config loading, client construction, hostname lookup, or HTTP.
- Recommendation and choice membership comparisons are exact strings, matching the Worker rule.
- Existing Worker responses without the two new fields remain deserializable through
  `#[serde(default)]`; existing integration response fixtures cover that compatibility path.
- Checklist payloads omit both optional fields rather than serializing null values, preserving
  rollout compatibility.
- No unrelated production behavior or documentation compatibility note was removed.
