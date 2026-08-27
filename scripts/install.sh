#!/bin/sh
set -eu

toolchain="nightly-2026-06-30"
repository="https://github.com/themixednuts/mlua-typegen"

if ! command -v rustup >/dev/null 2>&1; then
    echo "error: rustup is required; install Rust from https://rustup.rs and retry" >&2
    exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
    echo "error: cargo is not on PATH; restart the shell after installing rustup" >&2
    exit 1
fi

case "$(uname -s)" in
    Linux|Darwin)
        rustup toolchain install "$toolchain" --profile minimal \
            --component rustc-dev \
            --component llvm-tools-preview
        ;;
    *)
        rustup toolchain install "$toolchain" --profile minimal \
            --component rustc-dev
        ;;
esac

cargo "+$toolchain" install \
    --git "$repository" \
    --locked \
    --force

cargo mlua-typegen --version
echo "Run 'cargo mlua-typegen' in any Cargo workspace."
