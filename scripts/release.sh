#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
ver="${1:-}"
ver="${ver#v}"
if [[ ! "$ver" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; then
    echo 'error: version must be X.Y.Z' >&2
    exit 1
fi
[[ "$(git branch --show-current)" == main ]] || { echo 'error: release from main' >&2; exit 1; }
[[ -z "$(git status --porcelain)" ]] || { echo 'error: commit or stash changes first' >&2; exit 1; }
git fetch upstream --tags
[[ "$(git rev-parse HEAD)" == "$(git rev-parse upstream/main)" ]] || { echo 'error: main must match upstream/main' >&2; exit 1; }
if git show-ref --verify --quiet "refs/tags/v$ver"; then
    echo "error: tag v$ver already exists" >&2
    exit 1
fi
# Python keeps this portable across GNU and BSD systems. Only change our package.
python3 - "$ver" <<'PY'
import pathlib, re, sys
path = pathlib.Path('cli/Cargo.toml')
source = path.read_text()
source, count = re.subn(r'(?m)^version = "[^"]+"$', f'version = "{sys.argv[1]}"', source, count=1)
assert count == 1, 'package version missing'
path.write_text(source)
PY
(cd cli && cargo check --offline)
just check
(cd cli && cargo build --locked --release)
git add cli/Cargo.toml cli/Cargo.lock
if ! git diff --cached --quiet; then
    git commit -m "chore: release v$ver"
fi
git tag -a "v$ver" -m "lam v$ver"
git push --atomic upstream main "v$ver"
echo "Pushed v$ver; GitHub Actions will publish the binaries."
