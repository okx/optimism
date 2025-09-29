FROM golang:1.24.4
RUN apt-get update && apt-get install -y \
    curl \
    git \
    jq \
    make \
    gcc \
    nodejs \
    npm \
    bash \
    && apt-get clean && rm -rf /var/lib/apt/lists/*
ARG TARGETARCH
RUN case ${TARGETARCH} in \
            "amd64") JUST_ARCH="x86_64" ;; \
            "arm64") JUST_ARCH="aarch64" ;; \
            *) echo "Unsupported architecture: ${TARGETARCH}" >&2; exit 1 ;; \
        esac && \
        curl -sSL "https://github.com/casey/just/releases/download/1.36.0/just-1.36.0-${JUST_ARCH}-unknown-linux-musl.tar.gz" | tar -xz -C /usr/local/bin just && \
        chmod +x /usr/local/bin/just

