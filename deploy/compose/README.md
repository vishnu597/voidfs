# voidfs with Docker Compose

`compose.yaml` runs voidfs-server from this repository's [Dockerfile](../../Dockerfile). The pool
lives in a Docker volume, or in your own bucket if you point it there.

## Start it

```bash
cd deploy/compose
cp example.env .env      # set VOIDFS_ACCESS_KEY_ID and VOIDFS_SECRET_ACCESS_KEY in it
docker compose up -d --build --wait
```

The S3 endpoint is then `http://127.0.0.1:9000`, with the key from `.env`. Every drive is a
bucket, for example:

```bash
aws --endpoint-url http://127.0.0.1:9000 s3 mb s3://photos
```

`.env` is ignored by git. `VOIDFS_PORT` moves the published port; keep it on `127.0.0.1` unless
a proxy with TLS is in front, because the traffic is plain HTTP.

## With a bucket beside it

`compose.versitygw.yaml` adds [versitygw](https://github.com/versity/versitygw), an S3 server over
a local volume, and keeps the pool there as a real deployment would keep it in S3 or R2. Set
`BUCKET_ACCESS_KEY_ID` and `BUCKET_SECRET_ACCESS_KEY` in `.env` (any values), then:

```bash
docker compose -f compose.yaml -f compose.versitygw.yaml up -d --build --wait
```

The server checks at start that the bucket refuses a second create of the same object (format
§7.2), which versitygw does. MinIO would serve the same purpose, but it no longer publishes images
or binaries; CI builds its last release from source instead. Direct uploads (protocol §4.11) are
off here: versitygw is reachable only inside the Compose network, so a client couldn't send shards
to it. A pool in your own bucket has them, where the bucket enforces what their URLs bind.

## In your own bucket

Set `VOIDFS_STORE=s3:<bucket>/<prefix>` and the `VOIDFS_S3_*` settings in `.env` (see
[example.env](example.env)), and use `compose.yaml` alone. For Cloudflare R2, the endpoint is
`https://<account>.r2.cloudflarestorage.com` and the region `auto`. For AWS S3, leave the
endpoint out and set the bucket's region.

## Health and metrics

The server's admin endpoints, `/healthz`, `/readyz` and `/metrics` (see the
[README](../../README.md#health-checks-and-metrics)), listen on `127.0.0.1:9001` inside the
container. The container is healthy once `/readyz` answers 200: the pool is open and the bucket
answers. To look for yourself:

```bash
docker compose exec voidfs curl -s http://127.0.0.1:9001/readyz
docker compose exec voidfs curl -s http://127.0.0.1:9001/metrics
```

Nothing on the admin port is authenticated, so it is not published, and shouldn't be. For
Prometheus in another container on the Compose network, set `VOIDFS_ADMIN_LISTEN=0.0.0.0:9001` in
`.env` and scrape `voidfs:9001`. Keep the port 9001, which the health check uses, and leave it out
of `ports:`.

`docker compose stop` (SIGTERM) stops the server gracefully: it stops taking connections, finishes
the requests in progress, and `/readyz` answers 503 meanwhile. A client following a drive's change
feed as Server-Sent Events keeps its connection open, so then Docker kills the server after its
10-second timeout, as before.

## Checking it

With it running, the conformance suite and the client checks work against it as against any
voidfs server:

```bash
set -a; . deploy/compose/.env; set +a
VOIDFS_ENDPOINT=http://127.0.0.1:9000 tests/interop/rclone_smoke.sh
```

CI brings up `compose.yaml` with `compose.versitygw.yaml` on every pull request, runs the
conformance suite against it, and checks the admin endpoints from inside the container and that
the host can't reach them.
