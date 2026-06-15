# Stage 1: install tools and cache dependencies
FROM rust:1.87.0 AS planner
WORKDIR /app

# Install nightly toolchain (needed to build the eBPF crate with -Z build-std)
RUN rustup toolchain install nightly --component rust-src

# Install cargo-binstall, then use it to grab the prebuilt bpf-linker binary.
# This avoids pulling in LLVM as a build dependency.
RUN curl -L --proto '=https' --tlsv1.2 -sSf \
    https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh \
    | bash
RUN cargo binstall --no-confirm bpf-linker

RUN cargo install cargo-chef
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Stage 2: cache dependencies
FROM rust:1.87.0 AS cacher
WORKDIR /app

RUN rustup toolchain install nightly --component rust-src
RUN curl -L --proto '=https' --tlsv1.2 -sSf \
    https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh \
    | bash
RUN cargo binstall --no-confirm bpf-linker

RUN cargo install cargo-chef
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Stage 3: build
FROM rust:1.87.0 AS builder
WORKDIR /app

RUN rustup toolchain install nightly --component rust-src
RUN curl -L --proto '=https' --tlsv1.2 -sSf \
    https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh \
    | bash
RUN cargo binstall --no-confirm bpf-linker

COPY . .
COPY --from=cacher /app/target target
COPY --from=cacher $CARGO_HOME $CARGO_HOME
RUN cargo build --release

# Stage 4: runtime
# Loading BPF programs requires elevated privileges.
# Run with: docker run --privileged  (or --cap-add CAP_BPF --cap-add CAP_PERFMON --cap-add CAP_SYS_ADMIN)
FROM ubuntu:22.04 AS runtime
WORKDIR /app

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y ca-certificates tzdata libssl3 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/scaphandre /usr/local/bin
ENTRYPOINT ["/usr/local/bin/scaphandre"]
