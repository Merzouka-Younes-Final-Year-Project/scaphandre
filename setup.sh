#!/bin/bash
# setup-ebpf.sh — dev environment setup for aya-rs eBPF projects
# Mirrors the Dockerfile: nightly + rust-src, cargo-binstall, bpf-linker, cargo-chef

set -euo pipefail

info()  { echo -e "\e[1;34m[INFO]\e[0m  $*"; }
ok()    { echo -e "\e[1;32m[OK]\e[0m    $*"; }
die()   { echo -e "\e[1;31m[ERROR]\e[0m $*" >&2; exit 1; }

# ── 1. Nightly toolchain + rust-src ─────────────────────────────────────────

info "Installing nightly toolchain with rust-src component..."
rustup toolchain install nightly --component rust-src
ok "nightly + rust-src ready"

# ── 2. cargo-binstall ────────────────────────────────────────────────────────

if cargo binstall --version &>/dev/null 2>&1; then
    ok "cargo-binstall already installed ($(cargo binstall --version))"
else
    info "Installing cargo-binstall..."
    curl -L --proto '=https' --tlsv1.2 -sSf \
        https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh \
        | bash
    ok "cargo-binstall installed"
fi

# ── 3. bpf-linker (via binstall — avoids building LLVM from source) ──────────

if command -v bpf-linker &>/dev/null; then
    ok "bpf-linker already installed"
else
    info "Installing bpf-linker..."
    cargo binstall --no-confirm bpf-linker
    ok "bpf-linker installed"
fi

# ── 4. cargo-chef ────────────────────────────────────────────────────────────

if cargo chef --version &>/dev/null 2>&1; then
    ok "cargo-chef already installed ($(cargo chef --version))"
else
    info "Installing cargo-chef..."
    cargo install cargo-chef
    ok "cargo-chef installed"
fi

# ── 5. System deps (Linux only) ──────────────────────────────────────────────

if [[ "$(uname)" == "Linux" ]]; then
    if command -v apt-get &>/dev/null; then
        info "Installing system deps via apt..."
        sudo apt-get update -q
        sudo DEBIAN_FRONTEND=noninteractive apt-get install -y \
            pkg-config libssl-dev ca-certificates
        ok "apt deps installed"
    elif command -v dnf &>/dev/null; then
        info "Installing system deps via dnf (Fedora)..."
        sudo dnf install -y openssl-devel
        ok "dnf deps installed"
    else
        echo "[WARN]  Unknown package manager — skipping system deps. Install openssl-devel/libssl-dev manually if the build fails."
    fi
fi

# ── Done ──────────────────────────────────────────────────────────────────────

echo ""
ok "eBPF dev environment ready."
echo ""
echo "  Toolchain : nightly (with rust-src)"
echo "  Linker    : bpf-linker"
echo "  Tools     : cargo-binstall, cargo-chef"
echo ""
echo "  Build eBPF crate  : cargo +nightly build -Z build-std=core --target bpfel-unknown-none --release"
echo "  Build userspace   : cargo build --release"
