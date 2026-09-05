#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -ne 1 ]]; then
  printf 'usage: %s APK\n' "$0" >&2
  exit 2
fi

apk="$1"
analyzer="${LAM_APKANALYZER:-apkanalyzer}"
manifest=$("$analyzer" manifest print "$apk")
registrars=$(sed -n 's/.*android:name="com.google.firebase.components:\(com.google.mlkit\.[^"]*\)".*/\1/p' <<<"$manifest")
if [[ -z "$registrars" ]]; then
  printf 'FAIL: APK has no ML Kit registrar metadata\n' >&2
  exit 1
fi

failed=0
while IFS= read -r registrar; do
  code=$("$analyzer" dex code --class "$registrar" "$apk")
  if grep -Eq '^\.method public constructor <init>\(\)V$' <<<"$code"; then
    printf 'PASS: %s has its reflective constructor\n' "$registrar"
  else
    printf 'FAIL: %s has no public no-argument constructor in APK\n' "$registrar" >&2
    failed=1
  fi
done <<<"$registrars"
exit "$failed"
