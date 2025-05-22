FROM debian:bookworm-slim

# Install base dependencies
RUN apt-get update && apt-get install -y \
    build-essential \
    curl \
    git \
    wget \
    llvm \
    lld \
    nasm \
    qemu-system-x86 \
    gcc-x86-64-linux-gnu \
    binutils-x86-64-linux-gnu \
    gdb \
    pkg-config \
    libssl-dev \
    ca-certificates \
    xz-utils \
    && rm -rf /var/lib/apt/lists/*

# Install Rust with rustup
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

RUN curl https://sh.rustup.rs -sSf | sh -s -- -y --default-toolchain nightly

# Add rust target and components for bare metal dev
RUN rustup target add x86_64-unknown-none \
    && rustup component add llvm-tools-preview \
    && cargo install cargo-binutils

# Install just task runner
RUN curl --proto '=https' --tlsv1.2 -sSf https://just.systems/install.sh | bash -s -- --to /usr/bin

# Install lazygit
RUN wget https://github.com/jesseduffield/lazygit/releases/download/v0.50.0/lazygit_0.50.0_Linux_x86_64.tar.gz && \
    tar -xzf lazygit_0.50.0_Linux_x86_64.tar.gz && \
    mv lazygit /usr/bin/ && \
    chmod +x /usr/bin/lazygit && \
    rm lazygit_0.50.0_Linux_x86_64.tar.gz README.md LICENSE

# Set working directory
WORKDIR /usr/src/project

# Set environment variables
ENV RUST_BACKTRACE=1

CMD ["bash"]
