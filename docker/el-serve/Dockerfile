# The EL daemon, containerized: zdbt-el-serve + the connector worker.
# Build from the repo root:
#   docker build -f docker/el-serve/Dockerfile -t zdbt-el-serve .
# Run with a project mounted at /project and a token:
#   docker run -p 7431:7431 -e ZDBT_EL_TOKEN=... -v $PWD:/project zdbt-el-serve
# In-container the bind is plaintext-with-token (--insecure-http);
# publish only to localhost (-p 127.0.0.1:7431:7431) or terminate TLS at
# your ingress for anything reachable from a network.

FROM rust:1.90-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p el_serve -p el_worker

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/zdbt-el-serve /usr/local/bin/
COPY --from=build /src/target/release/zdbt-el-worker /usr/local/bin/
EXPOSE 7431
ENTRYPOINT ["zdbt-el-serve", "--project", "/project", "--listen", "0.0.0.0:7431", "--insecure-http", "--worker", "/usr/local/bin/zdbt-el-worker"]
