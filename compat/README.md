# Compatibility suite

Tests that drive the official [PocketBase JavaScript SDK](https://github.com/pocketbase/js-sdk)
against a running server, to check that slonbase behaves like PocketBase. CI runs them against
both slonbase and the PocketBase release, so every expectation here is PocketBase's actual
behavior.

```sh
npm ci
# slonbase: a debug build and an empty database it may use
(cd .. && cargo build)
DATABASE_URL='postgres://postgres:postgres@127.0.0.1:5432/compat?sslmode=disable' ./run.sh slonbase
# PocketBase
POCKETBASE_BIN=/path/to/pocketbase ./run.sh pocketbase
# one file
./run.sh slonbase tests/auth.test.mjs
```

`run.sh` starts the server on a fresh data store (port `COMPAT_PORT`, default 8099), creates the
superuser and runs `node --test`; the server log is printed when a test fails. Each test file
creates its own collections with unique names.
