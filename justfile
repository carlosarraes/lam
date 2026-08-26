# lam task runner — `just --list` shows everything.

binary := "lam"
bin_dir := env_var_or_default("BIN_DIR", env_var("HOME") + "/.local/bin")
version := `grep -m1 '^version' cli/Cargo.toml | cut -d'"' -f2`
commit := `git rev-parse --short HEAD 2>/dev/null || echo unknown`

[private]
default:
    @just --list

# Build the release CLI and install it to ~/.local/bin (override with BIN_DIR); link the skill
build:
    cd cli && cargo build --release
    mkdir -p {{bin_dir}} ~/.claude/skills
    rm -f {{bin_dir}}/{{binary}} && cp cli/target/release/{{binary}} {{bin_dir}}/{{binary}}
    ln -sfn "$PWD/skill/lam" ~/.claude/skills/lam
    @echo "installed {{bin_dir}}/{{binary}} (v{{version}} {{commit}})"

# Same as build
install: build

# Run both test suites
test:
    cd cli && cargo test
    cd worker && npx tsc -p . && npx vitest run

# Everything a release needs green: fmt, clippy, tests
check:
    cd cli && cargo fmt --all --check && cargo clippy --all-targets -- -D warnings
    just test

# Deploy the Worker and apply pending D1 migrations
deploy-worker:
    cd worker && npx wrangler deploy && npx wrangler d1 migrations apply lam --remote

# Install lam on another machine from ~/.ssh/config: copies the binary when OS/arch match,
# otherwise syncs the source and builds there; then copies the CLI config and the skill.
# e.g. `just deploy mac`
deploy HOST: build
    #!/usr/bin/env bash
    set -euo pipefail
    ssh -o BatchMode=yes {{HOST}} 'mkdir -p ~/.local/bin ~/.claude/skills'
    remote=$(ssh -o BatchMode=yes {{HOST}} 'uname -sm')
    # Replace, never overwrite in place: macOS SIGKILLs a signed binary whose inode was rewritten.
    ssh -o BatchMode=yes {{HOST}} 'rm -f ~/.local/bin/{{binary}}'
    if [ "$remote" = "$(uname -sm)" ]; then
        scp -q cli/target/release/{{binary}} {{HOST}}:~/.local/bin/{{binary}}
    else
        echo "{{HOST}} is $remote; building from source there"
        rsync -aq --delete --exclude target --exclude node_modules --exclude .wrangler --exclude .dev.vars --exclude .git ./ {{HOST}}:.cache/lam-src/
        ssh -o BatchMode=yes {{HOST}} 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/.cache/lam-src/cli && cargo build --release -q && cp target/release/{{binary}} ~/.local/bin/{{binary}}'
    fi
    tar -C skill -cf - lam | ssh -o BatchMode=yes {{HOST}} 'rm -rf ~/.claude/skills/lam && tar -C ~/.claude/skills -xf -' 
    case "$remote" in
        Darwin*) cfg='Library/Application Support/lam' ;;
        *)       cfg='.config/lam' ;;
    esac
    ssh -o BatchMode=yes {{HOST}} "mkdir -p \"\$HOME/$cfg\""
    scp -q ~/.config/lam/config.toml "{{HOST}}:$cfg/config.toml"
    ssh -o BatchMode=yes {{HOST}} '~/.local/bin/{{binary}} --version && ~/.local/bin/{{binary}} list >/dev/null && echo "  config ok"'

# Deploy to every concrete Host in ~/.ssh/config (skips ones that are unreachable)
deploy-all: build
    #!/usr/bin/env bash
    set -euo pipefail
    for host in $(awk '/^Host /{for(i=2;i<=NF;i++) if ($i !~ /[*?!.]/) print $i}' ~/.ssh/config | sort -u); do
        echo "== $host"
        just deploy "$host" || echo "   skipped $host"
    done

# Worker + every machine: the one command after any change
sync: deploy-worker deploy-all

# Print version, commit and install dir
info:
    @echo "lam v{{version}} ({{commit}})"
    @echo "  install dir: {{bin_dir}}"
    @echo "  worker:      $(grep -o '"name": *"[^"]*"' worker/wrangler.jsonc | head -1)"

# Remove build artifacts
clean:
    cd cli && cargo clean
    rm -rf worker/.wrangler
