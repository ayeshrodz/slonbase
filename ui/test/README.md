# Dashboard checks

`run.sh` starts a fresh slonbase, seeds a small fixed data set and runs `check.mjs` on the
dashboard's main screens (sign-in, overview, records, a record in the editor, schema, API rules,
settings), in light and dark:

- **Accessibility:** [axe-core](https://github.com/dequelabs/axe-core) with the WCAG 2 A and AA
  rules. A serious or critical violation (text contrast below 4.5:1, a control without a label,
  ...) fails the check.
- **Screenshots:** each screen is compared with `baselines/<screen>-<theme>.png`; more than 1% of
  the pixels differing fails it, and the screenshot and a diff image go to `out/` (CI uploads
  them). Moving parts (animations, the live dot) are hidden; the overview is checked by axe only.

```sh
cd ui/test && npm ci && npx playwright install chromium
DATABASE_URL='postgres://...an empty database...' ./run.sh            # check
DATABASE_URL='postgres://...an empty database...' ./run.sh --update   # after a deliberate change
```

When a change to the dashboard is deliberate, update the baselines with `--update` and commit
them with the change (or take the screenshots CI uploaded, when it renders differently from
your machine).
