# Security policy

## Supported versions

Only the latest release gets fixes.

## Reporting a vulnerability

Please **don't open a public issue** for security problems. Use GitHub's
[private vulnerability reporting](https://github.com/nev-minhnh/Phemio/security/advisories/new)
instead, with the steps to reproduce and the version you tested.

Areas that matter most:

- **Link** — pairing, the encrypted session between your PCs, and what a PC on the same network that
  isn't paired can make Phemio do;
- **the transparency helper** (`mglass/`), which runs inside Explorer;
- anything that reads files or the clipboard.

You'll get an answer as soon as possible, and credit in the release notes if you'd like it.
