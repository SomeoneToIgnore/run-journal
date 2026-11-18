#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSHOP="$SCRIPT_DIR/../runvil-workshop/target/debug"
GATEWAY="${RUNVIL_GATEWAY_BIN:-$WORKSHOP/runvil-gateway}"

absolute() {
    echo "$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
}

PROMPT="$(absolute "${1:-$SCRIPT_DIR/prompts/agent.md}")"
JOURNAL="$(absolute "${2:-$SCRIPT_DIR/$(basename "${PROMPT%.md}").db}")"

if [ -z "${RUNVIL_GATEWAY_URL:-}" ]; then
    if ! "$GATEWAY" diagnostics | grep -Eq '"running":[[:space:]]*true'; then
        echo "run.sh: $GATEWAY reports no running Gateway; start Workshop, or set RUNVIL_GATEWAY_URL and RUNVIL_GATEWAY_API_KEY" >&2
        exit 1
    fi
    url="$("$GATEWAY" --print-url)"
    export RUNVIL_GATEWAY_URL="${url%%/auth*}/v1"
    export RUNVIL_GATEWAY_API_KEY="${url#*key=}"
fi

if [ -z "${RUNVIL_PLUGINS+set}" ] && [ -d "$WORKSHOP/plugins" ]; then
    export RUNVIL_PLUGINS="$WORKSHOP/plugins"
elif [ -z "${RUNVIL_PLUGINS:-}" ]; then
    unset RUNVIL_PLUGINS
fi

if [ -z "${RUNVIL_FILES+set}" ] && [ -n "${HOME:-}" ] && [ -d "$HOME/Downloads" ]; then
    RUNVIL_FILES="$HOME/Downloads"
fi

if [ -n "${RUNVIL_FILES:-}" ]; then
    files="$(cd "$RUNVIL_FILES" && pwd)"
    export RUNVIL_FILES="$files"
else
    unset RUNVIL_FILES
fi

cd "$SCRIPT_DIR"
exec cargo run --quiet -p resumable -- "$JOURNAL" "$PROMPT"
