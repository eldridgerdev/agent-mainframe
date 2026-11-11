# Image for AMF "run in container" projects. Built on demand by AMF
# (`container::image::ensure_image`) from this embedded copy; the same image
# serves the clone step, the agent/terminal sessions and the egress proxy.
FROM node:22-bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        git ca-certificates curl openssh-client tinyproxy bash less procps \
    && rm -rf /var/lib/apt/lists/*

RUN npm install -g @anthropic-ai/claude-code @openai/codex

# AMF runs the container as the host uid with no passwd entry. A 1777 home is
# what lets a fresh named volume mounted here be written by that uid.
RUN mkdir -m 1777 /home/sandbox
ENV HOME=/home/sandbox
