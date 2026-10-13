# Admin dashboard

`dist/` is the prebuilt admin UI of [PocketBase](https://github.com/pocketbase/pocketbase)
(`ui/dist` at commit `6f370ea7`), used unchanged under its MIT license (`LICENSE.md`). slonbase
implements the API it uses and serves it at `/_/`; the files are compressed and embedded into
the binary at build time (`build.rs`).

To update it, copy a newer `ui/dist` from PocketBase and check the dashboard against slonbase
(`tests/dashboard.rs` and a manual pass), since newer versions may use endpoints slonbase
doesn't have yet.
