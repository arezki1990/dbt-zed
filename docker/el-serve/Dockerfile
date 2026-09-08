# The EL daemon, containerized: zdbt-el-serve + the connector worker.
# Build from the repo root:
#   docker build -f docker/el-serve/Dockerfile -t zdbt-el-serve .
# Run with a project mounted at /project and a token:
#   docker run -p 127.0.0.1:7431:7431 -e ZDBT_EL_TOKEN=... -v $PWD:/project zdbt-el-serve
# In-container the bind is plaintext-with-token (--insecure-http);
# publish only to localhost (-p 127.0.0.1:7431:7431) or terminate TLS at
# your ingress for anything reachable from a network.

FROM rust:1.97-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends cmake python3 \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
# Only what the carve-out needs: the three crates, the root manifests,
# and the generator.
COPY Cargo.toml Cargo.lock LICENSE-GPL rust-toolchain.toml ./
COPY crates/el_engine crates/el_engine
COPY crates/el_worker crates/el_worker
COPY crates/el_serve crates/el_serve
COPY deploy/el-serve/standalone.py deploy/el-serve/standalone.py
RUN python3 deploy/el-serve/standalone.py /src /el && cd /el \
    && cargo build --release -p el_serve -p el_worker

FROM debian:bookworm-slim
# libaio: what Oracle Instant Client needs when the thick Oracle driver is
# used (Oracle 10g / 11g targets). The client itself is not redistributable:
# mount it in and name it, e.g.
#   -v /opt/oracle/instantclient_19_26:/opt/oracle/instantclient:ro
#   -e ZDBT_EL_ORACLE_CLIENT_DIR=/opt/oracle/instantclient
#   -e LD_LIBRARY_PATH=/opt/oracle/instantclient
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libaio1 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /el/target/release/zdbt-el-serve /usr/local/bin/
COPY --from=build /el/target/release/zdbt-el-worker /usr/local/bin/
EXPOSE 7431
ENTRYPOINT ["zdbt-el-serve", "--project", "/project", "--listen", "0.0.0.0:7431", "--insecure-http", "--worker", "/usr/local/bin/zdbt-el-worker"]
