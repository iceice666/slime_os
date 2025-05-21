FROM rust:1.87-bullseye

# Install essential tools for OS development
RUN apt-get update && apt-get install -y \
    build-essential \
    curl \
    git \
    llvm \
    lld \
    nasm \
    qemu-system-x86 \
    gcc-x86-64-linux-gnu \
    binutils-x86-64-linux-gnu \
    gdb \
    && rm -rf /var/lib/apt/lists/*

# Install rust components needed for OS dev
RUN rustup component add rust-src rustfmt clippy llvm-tools-preview && \
    rustup target add x86_64-unknown-none

# Install just the task runner
RUN curl --proto '=https' --tlsv1.2 -sSf https://just.systems/install.sh | bash -s -- --to /usr/bin

# Install lazygit
RUN wget https://github.com/jesseduffield/lazygit/releases/download/v0.50.0/lazygit_0.50.0_Linux_x86_64.tar.gz && \
    tar -xzf lazygit_0.50.0_Linux_x86_64.tar.gz && \
    mv lazygit /usr/bin/ && \
    chmod +x /usr/bin/lazygit && \
    rm lazygit_0.50.0_Linux_x86_64.tar.gz README.md LICENSE

# Set up working directory
WORKDIR /usr/src/project

# Set environment variables
ENV RUST_BACKTRACE=1

CMD ["bash"]