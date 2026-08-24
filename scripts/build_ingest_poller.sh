#!/usr/bin/env bash
# Build the ingest-poller (agent-side inbox drain) on this WSL box.
# Same OpenSSL/zstd env as scripts/build_poller.sh, but in the ingest-poller dir.
set -euo pipefail
cd "$(dirname "$0")/../client/ingest-poller"

export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"
export PKG_CONFIG_PATH="$HOME/.local/openssl-dev/usr/lib/x86_64-linux-gnu/pkgconfig"
export CFLAGS="-I$HOME/.local/openssl-dev/usr/include"
export LDFLAGS="-L$HOME/.local/openssl-dev/usr/lib/x86_64-linux-gnu -lz -lzstd"

# Pin the shared target dir so the binary lands at a stable path the s6 run
# script execs (mirrors where digest-poller's binary lives). Note: the repo
# lives at /opt/data/digital-pantry (NOT under $HOME=/opt/data/home).
export CARGO_TARGET_DIR="/opt/data/digital-pantry/client/digest-poller/target"

exec cargo build --release "$@"
