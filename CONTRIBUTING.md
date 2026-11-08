# Contributing to Phemio

Thanks for helping! Bug reports, ideas, translations and pull requests are all welcome.

## Reporting a bug

Open an issue with:

- what you did, what you expected and what happened instead;
- your Windows version and Phemio version (Settings → About);
- **Settings → About → Copy diagnostics** pasted into the issue (it holds the version, the island's
  state and the recent log — check it for anything you'd rather not share first).

## Pull requests

1. Fork, then branch from `main`.
2. Build the helper and the app (see [README → Build from source](README.md#build-from-source)).
3. Keep changes focused and match the surrounding style (long lines are fine; comments say why).
4. Text shown in the app goes through `tr("…")` in plain English. Run `python3 i18n/missing.py`,
   add the new strings to `i18n/keys.json` and each `i18n/tr_<lang>.json` (leave a translation out
   if you don't know it — English is shown instead), then `python3 i18n/gen.py`.
5. Describe what changed and how you tested it. The CI build must pass.

## Translations

Each language is one JSON file in `i18n/` mapping the English text to the translation. Improving a
translation is just editing that file and running `python3 i18n/gen.py`.

## License of contributions

Phemio is dual-licensed under MIT or Apache-2.0. Unless you explicitly state otherwise, any
contribution intentionally submitted for inclusion in Phemio by you, as defined in the Apache-2.0
license, shall be dual-licensed as above, without any additional terms or conditions.
