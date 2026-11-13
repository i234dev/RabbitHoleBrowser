# Delivery conventions

- All commits must use `i234dev` as both author and committer, with empty email
  addresses. Do not add co-author, contributor or assistant attribution trailers.
- Every new delivery published to `main` increments the workspace patch version.
  Patch numbers are integers: `0.1.9` is followed by `0.1.10`.
- Fetch the previous published revision, then use
  `python3 scripts/release-version.py prepare`. Repeating preparation before
  publication must keep the same version. See [the release procedure](docs/releasing.md).
- Keep workspace manifests, `Cargo.lock`, the README status heading, the binary
  version, and `about:home` consistent. Obtain runtime versions from Cargo;
  do not hardcode a shortened version in the interface.
- Display compatibility and performance measurements from their recorded sources,
  with their date, corpus and measurement conditions. Historical or partial
  measurements must remain identified as such.
- Validate changes before publishing, build artifacts from the delivered revision,
  and record the revision and checksums associated with each artifact.
