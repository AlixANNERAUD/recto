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

# The runtime base: Debian 13 like the builder, with glibc, libgcc, libstdc++, OpenSSL, zlib, CA
# certificates and the nonroot user (65532), but no shell, package manager or other programs.
FROM gcr.io/distroless/cc-debian13:nonroot@sha256:cb87acc574abe719b6e5c547384defa2f7b1f24af9605d3f618cf852495d8be2 AS base

# Debian's libsane1 pulls in perl, systemd, gnupg and image codecs that nothing here loads, so SANE
# is installed in this stage and only what recto and SANE load at run time goes to /rootfs, which
# the image lays over the base.
FROM debian:trixie-slim@sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2 AS rootfs

# sane-utils is only here for `scanimage -L` when diagnosing a scanner that does not show up.
RUN apt-get update \
    && apt-get install -y --no-install-recommends libsane1 sane-airscan sane-utils \
    && rm -rf /var/lib/apt/lists/*

COPY --from=base / /base
COPY --from=builder /out/recto /app/recto
# Keeps recto's own eSCL scanners out of SANE when they are advertised on the same network. Read
# after /etc/sane.d/airscan.conf, so a mounted airscan.conf leaves it in place.
COPY examples/sane/airscan.d/recto.conf /etc/sane.d/airscan.d/recto.conf

SHELL ["/bin/bash", "-euo", "pipefail", "-c"]
RUN <<'EOF'
mkdir /rootfs
# The multiarch folder, /usr/lib/x86_64-linux-gnu or /usr/lib/aarch64-linux-gnu.
libdir=$(dirname "$(dpkg -L libsane1 | grep '/libsane\.so\.1$')")
# libsane opens the backends of dll.conf with dlopen, and libraries they link load plugins in
# turn (gphoto2's camera and port drivers, libv4l's, libheif's decoders, OpenSSL's legacy
# provider, Kerberos's and SASL's), so ldd on the binary alone would miss them.
plugins=(
    "$libdir/sane" "$libdir/libgphoto2" "$libdir/libgphoto2_port" "$libdir/libv4l"
    "$libdir/libheif" "$libdir/ossl-modules" "$libdir/krb5" "$libdir/sasl2"
)
mapfile -t objects < <(
    printf '%s\n' /app/recto /usr/bin/scanimage
    find "${plugins[@]}" -type f -name '*.so*'
)
# The libraries they need, as a link (libsane.so.1) and the file it points to, under /usr since
# ldd reports them through the /lib link.
deps=$(ldd "${objects[@]}")
if grep 'not found' <<<"$deps"; then exit 1; fi
mapfile -t libraries < <(
    awk '$2 == "=>" && $3 ~ /^\// { print $3 } $1 ~ /^\// && $1 !~ /:$/ { print $1 }' <<<"$deps" \
        | sort -u \
        | while read -r library; do
            link="$(readlink -f "$(dirname "$library")")/$(basename "$library")"
            printf '%s\n' "$link" "$(readlink -f "$link")"
        done \
        | sort -u
)
# The base already has glibc, libgcc, libstdc++, OpenSSL and zlib: keep its copies.
for library in "${libraries[@]}"; do
    if [ ! -e "/base$library" ]; then cp -a --parents "$library" /rootfs; fi
done
# Their configuration and data: SANE's and sane-airscan's, libv4l's decompressors, Avahi's
# service names, the MIBs of libsnmp (used by the network backends), and fontconfig with its
# fonts for the PDFs poppler renders for escl.
cp -a --parents /usr/bin/scanimage "${plugins[@]}" "$libdir/libv4lconvert0" "$libdir/avahi" \
    /etc/sane.d /etc/snmp /usr/share/snmp /etc/fonts /usr/share/fontconfig /usr/share/fonts \
    /rootfs
# The default database (sqlite:///data/recto.db) lives here. It must exist in the image, owned by
# recto's user, since the root file system may be read-only; a new named volume mounted on it
# takes this ownership.
install -d -o 65532 -g 65532 /rootfs/data

# Fail the build unless every program, backend and plugin loads in the assembled image, which has
# no ld.so.cache and no shell: every library must resolve, with every symbol bound.
cp -a /base /check
cp -a /rootfs/. /check/
cp -a --parents /app/recto /check
loader=$(awk '$1 ~ /\/ld-linux/ { print $1; exit }' <<<"$deps")
for object in "${objects[@]}"; do
    if ! out=$(LD_WARN=yes LD_BIND_NOW=yes chroot /check "$loader" --list "$object" 2>&1) \
        || grep -E 'not found|undefined symbol|error while loading' <<<"$out"; then
        printf '%s does not load:\n%s\n' "$object" "$out" >&2
        exit 1
    fi
done
# And SANE works there: its test backend lists two devices.
mkdir /check/sane-test
echo test >/check/sane-test/dll.conf
SANE_CONFIG_DIR=/sane-test chroot /check /usr/bin/scanimage -L | grep "device \`test:0'"
chroot /check /app/recto --version
rm -rf /check
EOF

FROM base

LABEL org.opencontainers.image.title="recto" \
      org.opencontainers.image.description="Print and scan from a browser: CUPS and SANE behind local accounts or OpenID Connect." \
      org.opencontainers.image.source="https://github.com/AlixANNERAUD/recto" \
      org.opencontainers.image.licenses="Apache-2.0"

COPY --from=rootfs /rootfs /
COPY --from=builder /out/recto /app/recto
COPY --from=builder /out/site /app/site

ENV RECTO_LISTEN_ADDRESS=0.0.0.0:8080 \
    RECTO_SITE_ROOT=/app/site

USER 65532:65532
EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=6s --start-period=20s --retries=3 \
    CMD ["/app/recto", "healthcheck"]

ENTRYPOINT ["/app/recto"]
CMD ["serve"]
