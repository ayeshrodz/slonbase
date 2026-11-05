# The slonbase dashboard

The admin dashboard served at `/_/`. It's plain ES modules with no build step:
[Preact](https://preactjs.com) and [htm](https://github.com/developit/htm) are vendored in
`app/vendor` (licenses next to them), and `build.rs` embeds `app/` into the binary (text files
gzipped). Edit the files and rebuild.

- `app/main.js` — the shell: the sidebar (sections, collections, theme, account), command palette (⌘K / Ctrl K), routes
- `app/lib/` — the API client, store and router, components, icons and formatting
- `app/pages/` — one module per screen
- `app/app.css` — design tokens (light and dark), components and motion
- `app/fonts/` — the three typefaces (SIL Open Font License): Instrument Serif for titles and
  figures, Geist for the interface, Geist Mono for small-capital labels, ids and code

It talks to the server only through the public API, the same one the SDKs use.

`test/` checks the dashboard in CI: on a fresh install with a fixed data set it runs axe-core
(WCAG 2 A and AA) on the main screens in both themes and compares their screenshots with
`test/baselines/`. After an intended visual change, regenerate them with
`DATABASE_URL=... test/run.sh --update` (see [test/README.md](test/README.md)).
