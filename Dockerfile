FROM rust:1-slim AS build
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
COPY --from=build /app/target/release/explicrate /usr/local/bin/explicrate
ENV EXPLICRATE_CACHE=/data
VOLUME /data
EXPOSE 8080
CMD ["explicrate", "serve", "0.0.0.0:8080"]
