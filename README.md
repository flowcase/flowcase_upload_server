# flowcase_upload_server

Tiny HTTPS server that accepts chunked Dropzone-style uploads. Used inside
Flowcase droplet images to ship files into the session container.

## Build

```sh
cargo build --release
```

The binary lands at `target/release/flowcase_upload_server`.

## CLI

```
flowcase_upload_server [--ssl] --auth-token <user:pass>
                       [--port <u16>] [--upload-dir <path>]
```

| Flag | Default | Meaning |
|------|---------|---------|
| `--ssl` | `false` | Generate an in-memory self-signed cert for `localhost` and serve HTTPS. Mirrors Flask's `ssl_context="adhoc"`. |
| `--auth-token` | required | `user:pass` credentials. Every request must send `Authorization: Basic base64(user:pass)`. |
| `--port` | `4902` | Listen port. |
| `--upload-dir` | `$HOME/Uploads` | Where final files land. Created if missing. |

## Endpoint

`POST /upload` — multipart/form-data with these fields (matches Dropzone
defaults):

| Field | Meaning |
|-------|---------|
| `file` | Chunk bytes. The `filename` parameter is sanitized (alphanumeric + space/dot/underscore/hyphen). |
| `dzchunkindex` | 0-based chunk number. |
| `dzchunkbyteoffset` | Byte offset within the final file. |
| `dztotalfilesize` | Final file size. Verified on the last chunk. |
| `dztotalchunkcount` | Total number of chunks. |

Responses:

| Status | Body | Cause |
|--------|------|-------|
| 200 | `uploaded Chunk` | Chunk written. |
| 400 | `File already exists` | `dzchunkindex == 0` and a final file with that name already exists. |
| 400 | `No Space available` | `dzchunkindex == 0` and `statvfs` says the volume can't fit `dztotalfilesize`. |
| 403 | `Access Denied!` | Missing/wrong `Authorization`. |
| 403 | `Failed to decode auth 1` | `Authorization: Basic …` payload isn't valid base64. |
| 500 | `Size mismatch` | On the last chunk the assembled `.uploading` file size doesn't match `dztotalfilesize`. |

## Manual smoke test

```sh
mkdir -p /tmp/up-smoke
./target/release/flowcase_upload_server --ssl \
    --auth-token flowcase_user:test --port 14902 \
    --upload-dir /tmp/up-smoke &
PID=$!

dd if=/dev/urandom of=/tmp/payload.bin bs=1024 count=1
head -c 500 /tmp/payload.bin > /tmp/c0.bin
tail -c +501 /tmp/payload.bin > /tmp/c1.bin

curl -sk -u flowcase_user:test -X POST https://localhost:14902/upload \
    -F "dzchunkindex=0" -F "dzchunkbyteoffset=0" \
    -F "dztotalfilesize=1024" -F "dztotalchunkcount=2" \
    -F "file=@/tmp/c0.bin;filename=multi.bin"

curl -sk -u flowcase_user:test -X POST https://localhost:14902/upload \
    -F "dzchunkindex=1" -F "dzchunkbyteoffset=500" \
    -F "dztotalfilesize=1024" -F "dztotalchunkcount=2" \
    -F "file=@/tmp/c1.bin;filename=multi.bin"

md5 /tmp/payload.bin /tmp/up-smoke/multi.bin   # should match

kill $PID
```

To check Dropzone in a browser, point `https://localhost:14902/upload` at a
Dropzone instance configured with `chunking: true` and Basic auth.

## Tests

```sh
cargo test
```

15 tests cover CLI parsing, the rustls self-signed config builder, every
branch of the auth middleware, and five upload paths including the
30 B / 3-chunk happy-path acceptance from REFACTOR_PLAN.md T1B.4.

## License

See [LICENSE](LICENSE).
