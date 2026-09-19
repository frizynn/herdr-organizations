# Release checklist

Use this checklist before publishing Herdr Organizations as a public Herdr plugin.

- [ ] Confirm the repository is named `herdr-organizations` and its description and homepage match the README.
- [ ] Keep the manifest id `herdr-projects` and display name `Herdr Organizations`. The id preserves the existing plugin config and project store.
- [ ] For an upstream replacement, uninstall the upstream plugin before linking this checkout so both registrations never write the same store:

  ```sh
  herdr plugin uninstall herdr-projects
  cargo build --release --locked
  herdr plugin link .
  ```

- [ ] Confirm the Herdr plugin list shows one `herdr-projects` registration named `Herdr Organizations`, with the existing projects and settings still available.
- [ ] Add the GitHub topic `herdr-plugin` after the repository is public so the marketplace can discover its manifest.
- [ ] Verify a clean install from the public repository builds with `cargo build --release --locked` and registers the expected actions and panes.
- [ ] Walk through [manual validation](manual-test.md), including the checks that require a Herdr client and installed agent CLIs.
- [ ] Exercise another agent kind, such as Gemini, and record the result in `docs/herdr-notes.md`.
- [ ] Keep the crate version, manifest version and release tag aligned.
- [ ] Decide whether to publish prebuilt release binaries so users can install without a Rust toolchain.
- [ ] Generalize client-specific notes before publication. Tracked files must not contain personal absolute paths or machine-only credentials.
