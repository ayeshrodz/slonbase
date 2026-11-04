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

## Differential runs

`diff/run.sh` starts PocketBase and slonbase side by side on fresh data stores, seeds both the
same way (fixed record ids), sends them the same requests and compares their answers as JSON;
values that differ by nature (times, tokens, generated ids) are compared by type, and error
answers by their fields' validation codes. The requests cover reads, filters, expand, fields,
writes, view previews, sign-up, sign-in, confirm tokens, rules for signed-in users and batches
(126 of them). It also
compares the `:excerpt` modifier on random HTML documents (`FUZZ`, default 1000; `FUZZ_SEED`
picks them). CI runs it on every push.

```sh
POCKETBASE_BIN=/path/to/pocketbase \
DATABASE_URL='postgres://postgres:postgres@127.0.0.1:5432/diff?sslmode=disable' \
compat/diff/run.sh            # add --only <name> to run some requests, -v to list them all
```

