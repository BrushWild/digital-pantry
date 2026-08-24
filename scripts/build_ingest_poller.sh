#!/usr/bin/env bash
# Build the ingest-poller (agent-side inbox drain) on this WSL box.
# Same OpenSSL/zstd env as scripts/build_poller.sh, but in the ingest-poller dir.
set -euo pipefail
cd "$(dirname "$0")/../client/ingest-poller"

export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"
export PKG_CONFIG_PATH="$HOME/.local/openssl-dev/usr/lib/x86_64-linux-gnu/pkgconfig"
export CFLAGS="-I$HOME/.local/openssl-dev/usr/include"
export LDFLAGS="-L$HOME/.local/openssl-dev/usr/lib/x86_64-linux-gnu -lz -lzstd"

exec cargo build --release "$@"
