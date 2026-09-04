#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -lt 3 || "$#" -gt 4 ]]; then
  printf 'usage: %s APP_APK TEST_APK TEST_PACKAGE [TEST_CLASS]\n' "$0" >&2
  exit 2
fi

app_apk="$1"
test_apk="$2"
test_package="$3"
test_class="${4:-}"
adb_command="${LAM_ADB:-adb}"

"$adb_command" install -r "$app_apk"
"$adb_command" install -r -t "$test_apk"

instrument_arguments=(-r -w)
if [[ -n "$test_class" ]]; then
  instrument_arguments+=(-e class "$test_class")
fi

if ! output=$("$adb_command" shell am instrument "${instrument_arguments[@]}" \
  "$test_package/androidx.test.runner.AndroidJUnitRunner" 2>&1); then
  printf '%s\n' "$output" >&2
  exit 1
fi

printf '%s\n' "$output"

if [[ "$output" == *"FAILURES!!!"* ]] ||
  [[ "$output" == *"INSTRUMENTATION_ABORTED"* ]] ||
  [[ "$output" == *"INSTRUMENTATION_FAILED"* ]] ||
  [[ "$output" == *"Process crashed."* ]]; then
  printf 'Android instrumentation did not complete cleanly\n' >&2
  exit 1
fi

if ! grep -Eq '^OK \([1-9][0-9]* tests?\)$' <<<"$output"; then
  printf 'Android instrumentation did not report a passing test count\n' >&2
  exit 1
fi

if ! grep -Fq 'INSTRUMENTATION_CODE: -1' <<<"$output"; then
  printf 'Android instrumentation did not report AndroidJUnitRunner success\n' >&2
  exit 1
fi
