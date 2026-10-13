# syntax=docker/dockerfile:1

# Static (musl) build, so the final image needs nothing but the binary.
FROM rust:1.97-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked \
    && cp target/release/slonbase /slonbase \
    && mkdir /data

FROM scratch
COPY --from=build /slonbase /slonbase
COPY --from=build --chown=65532:65532 /data /data
USER 65532:65532
ENV SLONBASE_BIND=0.0.0.0:8090 \
    SLONBASE_CONFIG=/data/slonbase.toml \
    SLONBASE_STORAGE_PATH=/data/storage
VOLUME /data
EXPOSE 8090
HEALTHCHECK --interval=10s --timeout=6s --start-period=5s CMD ["/slonbase", "health"]
ENTRYPOINT ["/slonbase"]
CMD ["serve"]
