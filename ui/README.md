# The slonbase dashboard

The admin dashboard served at `/_/`. It's plain ES modules with no build step:
[Preact](https://preactjs.com) and [htm](https://github.com/developit/htm) are vendored in
`app/vendor` (licenses next to them), and `build.rs` embeds `app/` into the binary (text files
gzipped). Edit the files and rebuild.

- `app/main.js` — the shell: top navigation, command palette (⌘K / Ctrl K), routes
- `app/lib/` — the API client, store and router, components, icons and formatting
- `app/pages/` — one module per screen
- `app/app.css` — design tokens (light and dark), components and motion

It talks to the server only through the public API, the same one the SDKs use.
