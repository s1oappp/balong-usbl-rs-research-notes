#!/usr/bin/env bash
# build.sh — сборка usbl-rs в usblsafe-327x-rs.bin
# Требования: rustup (stable, target armv7a-none-eabi), rust-lld (в комплекте).
set -e
cd "$(dirname "$0")"
. "$HOME/.cargo/env"

echo "=== usbl-rs: сборка (armv7a-none-eabi, ARM-режим, soft-float) ==="
cargo build --release --target armv7a-none-eabi

echo ""
echo "=== Упаковка образа (формат usblsafe-327x.bin) ==="
python3 tools/pack.py target/armv7a-none-eabi/release/usbl-rs usblsafe-327x-rs.bin
