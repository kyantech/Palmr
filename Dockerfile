FROM node:24-bookworm-slim AS web-builder

WORKDIR /workspace

COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY apps/web/package.json apps/web/package.json
RUN corepack enable && pnpm install --frozen-lockfile

COPY apps/web apps/web
RUN pnpm --filter web build

FROM rust:1.97.1-bookworm AS rust-builder

WORKDIR /workspace

COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY apps/server apps/server
COPY --from=web-builder /workspace/apps/web/dist apps/web/dist
RUN cargo build --release --locked --package palmr-server --bin palmr && mkdir /data-root

FROM gcr.io/distroless/cc-debian12:nonroot

COPY --from=rust-builder /workspace/target/release/palmr /usr/local/bin/palmr
COPY --from=rust-builder --chown=10001:10001 /data-root /data

USER 10001:10001
EXPOSE 5487
VOLUME ["/data"]
ENTRYPOINT ["/usr/local/bin/palmr"]
