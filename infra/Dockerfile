# Procurali free-MVP application image (P18-T01; serving binary merged in
# P21-T01/P21-T02).
#
# Pinned builder and runtime stages with digest-pinned bases (pulled
# 2026-10-11; re-resolve both tag and digest together on upgrade, never
# `latest`). Lockfile builds only (`--locked --release`). The context
# carries the binary crate, the compiled web bundle, migrations, and
# the public API contract — nothing else (see infra/.dockerignore).
# Non-root runtime, exec-form entrypoint, SIGTERM shutdown path.
# The health check reuses the binary itself (`--health-check`), so the
# image needs no HTTP client and no extra package.

# --- builder: exact locked release binary ---
# Context is the repository root. The crate keeps its backend/ + web/
# sibling layout because lib code includes the public template by
# relative path at compile time.
FROM rust:1.89-bookworm@sha256:948f9b08a66e7fe01b03a98ef1c7568292e07ec2e4fe90d88c07bb14563c84ff AS builder
WORKDIR /build
COPY backend/Cargo.toml backend/Cargo.lock backend/rust-toolchain.toml ./backend/
COPY backend/src ./backend/src
COPY backend/migrations ./backend/migrations
COPY web/public/request-template.html ./web/public/request-template.html
RUN cd backend && cargo build --locked --release --bin procurali-backend

# --- runtime: minimal non-root service image ---
FROM debian:bookworm-slim@sha256:7c7b2c966bc9ee8cedfeef67e0e279108992c77681fa595db4a9d65c06ccc587 AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN useradd --create-home --shell /usr/sbin/nologin appuser
WORKDIR /app
COPY --from=builder /build/backend/target/release/procurali-backend /app/procurali-backend
COPY backend/migrations /app/migrations
COPY web/dist /app/web-dist
COPY contracts/openapi.yaml /app/openapi.yaml
RUN chown -R appuser:appuser /app
USER appuser
ENV PROCURALI_ENV=production
ENV APP_ASSETS_DIR=/app/web-dist
ENV APP_BIND_ADDRESS=0.0.0.0:8080
EXPOSE 8080
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/app/procurali-backend", "--health-check"]
ARG GIT_SHA=unknown
ARG BUILD_DATE=unknown
LABEL org.opencontainers.image.revision="${GIT_SHA}" \
      org.opencontainers.image.created="${BUILD_DATE}" \
      org.opencontainers.image.title="procurali-backend"
ENTRYPOINT ["/app/procurali-backend"]
