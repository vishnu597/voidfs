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
or binaries; CI builds its last release from source instead.

## In your own bucket

Set `VOIDFS_STORE=s3:<bucket>/<prefix>` and the `VOIDFS_S3_*` settings in `.env` (see
[example.env](example.env)), and use `compose.yaml` alone. For Cloudflare R2, the endpoint is
`https://<account>.r2.cloudflarestorage.com` and the region `auto`. For AWS S3, leave the
endpoint out and set the bucket's region.

## Health

The container is healthy once the server answers HTTP: an unsigned request gets `403`. A health
endpoint and metrics that don't share the S3 port are still to come
([PARITY.md §7](../../docs/PARITY.md#7-step-by-step-plan), step 2).

## Checking it

With it running, the conformance suite and the client checks work against it as against any
voidfs server:

```bash
set -a; . deploy/compose/.env; set +a
VOIDFS_ENDPOINT=http://127.0.0.1:9000 tests/interop/rclone_smoke.sh
```

CI brings up `compose.yaml` with `compose.versitygw.yaml` and runs the conformance suite against
it on every pull request.
