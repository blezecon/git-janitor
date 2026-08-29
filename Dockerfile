FROM docker.io/nixos/nix@sha256:617d914dba5384bf75adf17081583b69371031ec7defce36c34c5fa14fc819b0

RUN mkdir -p /root/.config/nix && \
    echo "experimental-features = nix-command flakes" >> /root/.config/nix/nix.conf

WORKDIR /src

# Copy project files and build
COPY . .
RUN mkdir -p /usr/local/bin && \
    nix build .#default && \
    cp result/bin/* /usr/local/bin/

# Default command
CMD ["git-janitor"]
