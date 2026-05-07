FROM debian:bookworm-slim

ENV DEBIAN_FRONTEND=noninteractive \
    HOME=/home/amf \
    LANG=C.UTF-8 \
    TERM=xterm-256color \
    COLORTERM=truecolor \
    AMF_INSTALL_ROOT=/opt/amf

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash \
        ca-certificates \
        curl \
        git \
        jq \
        less \
        nodejs \
        npm \
        procps \
        tar \
    && npm install -g @anthropic-ai/claude-code \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --create-home --shell /bin/bash amf \
    && mkdir -p /opt/amf \
    && chown -R amf:amf /opt/amf /home/amf

WORKDIR /home/amf

COPY docker/no-tmux/install-amf-release.sh /usr/local/bin/amf-install-release
COPY docker/no-tmux/entrypoint.sh /usr/local/bin/amf-no-tmux-entrypoint

RUN chmod 755 \
        /usr/local/bin/amf-install-release \
        /usr/local/bin/amf-no-tmux-entrypoint

USER amf

ENTRYPOINT ["/usr/local/bin/amf-no-tmux-entrypoint"]
CMD ["bash"]
