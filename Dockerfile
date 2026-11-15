# syntax=docker/dockerfile:1.7

# The build uses the toolchain pinned in mise.toml (Rust, cargo-leptos, wasm-bindgen), like CI.
# The builder and the runtime share one Debian release so the binary links against the libsane
# it will run with.
FROM debian:trixie-slim@sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2 AS builder

# Updating mise means updating the checksums below (SHASUMS256.txt of its release).
ARG MISE_VERSION=2026.10.18
ARG TARGETARCH

# libsane-dev and pkg-config let the libsane crate link; libclang-dev is for its bindgen.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates curl git build-essential libsane-dev libclang-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*

# The mise binary is checked against the release's checksums before it runs.
RUN case "$TARGETARCH" in \
        amd64) arch=x64; sha256=e97c38973336aa2ff350fea50f47b412b266fe263b102b7200ff1e9b118c6942 ;; \
        arm64) arch=arm64; sha256=3a3c72376687ba21a588d0f5a2c88c76f483fb6cdece7b0fc43d5e3132b08a52 ;; \
        *) echo "unsupported architecture: $TARGETARCH" >&2; exit 1 ;; \
    esac \
    && curl -fsSL --retry 5 --retry-all-errors \
        "https://github.com/jdx/mise/releases/download/v${MISE_VERSION}/mise-v${MISE_VERSION}-linux-${arch}" \
        -o /usr/local/bin/mise \
    && echo "$sha256  /usr/local/bin/mise" | sha256sum --check --strict \
    && chmod +x /usr/local/bin/mise

ENV MISE_ENV=ci \
    MISE_DATA_DIR=/opt/mise \
    MISE_YES=1 \
    MISE_HTTP_RETRIES=5 \
    PATH=/opt/mise/shims:$PATH \
    CARGO_HOME=/opt/cargo \
    CARGO_TERM_COLOR=never \
    CARGO_INCREMENTAL=0

WORKDIR /src

# Tools first, so they are cached until mise.toml changes.
COPY mise.toml mise.ci.toml ./
RUN mise trust --all && mise install

# `mise run build` is a release build without the `dev-mocks` feature, which only `mise run dev`
# enables.
COPY . .
RUN --mount=type=cache,target=/opt/cargo/registry \
    --mount=type=cache,target=/opt/cargo/git \
    --mount=type=cache,target=/src/target,id=recto-target-${TARGETARCH} \
    mise run build \
    && mkdir -p /out \
    && cp target/release/recto /out/recto \
    && cp -r target/site /out/site

# The release archive's contents, exported with `--target artifact --output type=local`.
FROM scratch AS artifact
COPY --from=builder /out/recto /recto
COPY --from=builder /out/site /site

FROM debian:trixie-slim@sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2

LABEL org.opencontainers.image.title="recto" \
      org.opencontainers.image.description="Print and scan from a browser: CUPS and SANE behind OpenID Connect." \
      org.opencontainers.image.source="https://github.com/AlixANNERAUD/recto" \
      org.opencontainers.image.licenses="Apache-2.0"

# sane-utils is only here for `scanimage -L` when diagnosing a scanner that does not show up.
RUN apt-get update \
    && apt-get install -y --no-install-recommends libsane1 sane-airscan sane-utils ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /out/recto /app/recto
COPY --from=builder /out/site /app/site

ENV RECTO_LISTEN_ADDRESS=0.0.0.0:8080 \
    RECTO_SITE_ROOT=/app/site

# The default database (sqlite:///data/recto.db) lives here. It must exist in the image, owned by
# recto's user, since the root file system may be read-only; a new named volume mounted on it
# takes this ownership.
RUN mkdir /data && chown 65532:65532 /data

USER 65532:65532
EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=6s --start-period=20s --retries=3 \
    CMD ["/app/recto", "healthcheck"]

ENTRYPOINT ["/app/recto"]
CMD ["serve"]
