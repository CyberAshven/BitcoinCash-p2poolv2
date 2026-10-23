# syntax=docker/dockerfile:1.20
#
# Bitcoin Cash Node (BCHN) for the P2Poolv2 Docker stacks.
#
# The image installs the official BCHN release tarball, pinned by version and
# by SHA-256. The checksums are the ones in BCHN's signed release notes
# (https://gitlab.com/bitcoin-cash-node/announcements/-/tree/master/release-sigs/29.2.0,
# SHA256SUMS.29.2.0.asc) and match the digests GitHub shows for the release
# assets. To upgrade, change BCHN_VERSION and both checksums together.
#
# The base image is pinned by its multi-arch index digest, so a rebuild gets
# the same Debian packages until the digest is changed on purpose.

ARG DEBIAN_IMAGE=debian:trixie-slim@sha256:62bd423aeee912639ca8ee681c2b762cb3e25137e0a92b20be455898a9f034ad

FROM ${DEBIAN_IMAGE} AS download

ARG TARGETARCH
ARG BCHN_VERSION=29.2.0
ARG BCHN_SHA256_AMD64=23bbe9ff749136a9b832a2a59023ce76a5391322c1ea2f4f24b5b91f035081bc
ARG BCHN_SHA256_ARM64=27e177e5a07dc78c2c0d18798ea2e3925111955509ecdc476e828e7ce8f46383

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

RUN set -eu; \
    case "${TARGETARCH}" in \
      amd64) arch=x86_64; sum="${BCHN_SHA256_AMD64}" ;; \
      arm64) arch=aarch64; sum="${BCHN_SHA256_ARM64}" ;; \
      *) echo "No BCHN release for ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    tarball="bitcoin-cash-node-${BCHN_VERSION}-${arch}-linux-gnu.tar.gz"; \
    curl -fsSL -o "/tmp/${tarball}" \
      "https://github.com/bitcoin-cash-node/bitcoin-cash-node/releases/download/v${BCHN_VERSION}/${tarball}"; \
    echo "${sum}  /tmp/${tarball}" | sha256sum -c -; \
    mkdir -p /opt/bchn; \
    tar -xzf "/tmp/${tarball}" -C /opt/bchn --strip-components=1; \
    rm "/tmp/${tarball}"

FROM ${DEBIAN_IMAGE}

ARG BCHN_VERSION=29.2.0

COPY --from=download /opt/bchn/bin/bitcoind /opt/bchn/bin/bitcoin-cli /usr/local/bin/

RUN useradd --system --home-dir /data --shell /usr/sbin/nologin bchn \
    && mkdir -p /data /etc/bchn \
    && chown bchn:bchn /data

COPY bitcoin-regtest.conf bitcoin-chipnet.conf bitcoin-mainnet.conf /etc/bchn/
COPY bcli bootstrap.sh healthcheck.sh /usr/local/bin/
RUN chmod 0755 /usr/local/bin/bcli /usr/local/bin/bootstrap.sh /usr/local/bin/healthcheck.sh

USER bchn
VOLUME ["/data"]

# NETWORK picks the configuration file: regtest, chipnet or mainnet.
ENV NETWORK=regtest

# P2P and RPC ports: regtest 18444/18443, chipnet 48333/48332,
# mainnet 8333/8332. ZMQ hashblock is 28332 on every network.
EXPOSE 18443 18444 48332 48333 8332 8333 28332

LABEL org.opencontainers.image.title="Bitcoin Cash Node ${BCHN_VERSION} for P2Poolv2"
LABEL org.opencontainers.image.source=https://github.com/CyberAshven/BitcoinCash-p2poolv2
LABEL org.opencontainers.image.version="${BCHN_VERSION}"

HEALTHCHECK --interval=10s --timeout=5s --start-period=120s --retries=6 \
    CMD ["/usr/local/bin/healthcheck.sh"]

ENTRYPOINT ["/bin/sh", "-c", "exec bitcoind -conf=/etc/bchn/bitcoin-${NETWORK}.conf -datadir=/data \"$@\"", "bitcoind"]
