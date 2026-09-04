#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
runner="$script_dir/run-instrumentation-tests.sh"
test_dir="$(mktemp -d)"
trap 'rm -rf "$test_dir"' EXIT

fake_adb="$test_dir/adb"
cat > "$fake_adb" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "${FAKE_ADB_LOG:?}"
case "${FAKE_ADB_SCENARIO:?}" in
  success)
    if [[ "$1" == "install" ]]; then printf 'Success\n'; exit 0; fi
    cat <<'OUT'
OK (2 tests)
INSTRUMENTATION_CODE: -1
OUT
    ;;
  failure)
    if [[ "$1" == "install" ]]; then printf 'Success\n'; exit 0; fi
    cat <<'OUT'
FAILURES!!!
Tests run: 2,  Failures: 1
INSTRUMENTATION_CODE: -1
OUT
    ;;
  abort)
    if [[ "$1" == "install" ]]; then printf 'Success\n'; exit 0; fi
    cat <<'OUT'
INSTRUMENTATION_ABORTED: Process crashed.
INSTRUMENTATION_CODE: 0
OUT
    ;;
esac
EOF
chmod +x "$fake_adb"

touch "$test_dir/app.apk" "$test_dir/test.apk"
adb_log="$test_dir/adb.log"

FAKE_ADB_SCENARIO=success FAKE_ADB_LOG="$adb_log" LAM_ADB="$fake_adb" \
  "$runner" "$test_dir/app.apk" "$test_dir/test.apk" dev.example.test \
  >/dev/null
if grep -Fq -- '-e class' "$adb_log"; then
  printf 'runner filtered the default full instrumentation suite\n' >&2
  exit 1
fi

: > "$adb_log"
FAKE_ADB_SCENARIO=success FAKE_ADB_LOG="$adb_log" LAM_ADB="$fake_adb" \
  "$runner" "$test_dir/app.apk" "$test_dir/test.apk" dev.example.test dev.example.ExampleTest \
  >/dev/null
if ! grep -Fq -- '-e class dev.example.ExampleTest' "$adb_log"; then
  printf 'runner ignored the requested focused test class\n' >&2
  exit 1
fi

for scenario in failure abort; do
  if FAKE_ADB_SCENARIO="$scenario" FAKE_ADB_LOG="$adb_log" LAM_ADB="$fake_adb" \
    "$runner" "$test_dir/app.apk" "$test_dir/test.apk" dev.example.test dev.example.ExampleTest \
    >/dev/null 2>&1; then
    printf 'runner accepted %s output\n' "$scenario" >&2
    exit 1
  fi
done

printf 'strict instrumentation runner contract passed\n'
