#!/usr/bin/env bash
# Build the digest-poller on this WSL box (non-root, no apt).
#
# The spacetimedb-sdk pulls in `native-tls` -> `openssl-sys`, which needs a
# system OpenSSL. We use the prebuilt 3.5.6 dev tree at ~/.local/openssl-dev.
# (Its generated headers — opensslconf.h, configuration.h — were missing and
# were copied in from /tmp/openssl-3.5.6, same version, in an earlier session.)
#
# zstd: only libzstd.so.1 exists system-wide, so libzstd.so is symlinked into
# ~/.local/compress-dev/lib next to libz.so, and both are linked explicitly.
set -euo pipefail
cd "$(dirname "$0")/../client/digest-poller"

export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"
export PKG_CONFIG_PATH="$HOME/.local/openssl-dev/usr/lib/x86_64-linux-gnu/pkgconfig"
export CFLAGS="-I$HOME/.local/openssl-dev/usr/include"
# -lz -lzstd: flate2/zstd link against the system libs (see symlink above).
export LDFLAGS="-L$HOME/.local/openssl-dev/usr/lib/x86_64-linux-gnu -lz -lzstd"

exec cargo build --release "$@"
