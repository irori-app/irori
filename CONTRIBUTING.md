# Contributing to Irori

Thanks for helping. Bug reports, translations, docs and code are all welcome. This
page is a short introduction. [AGENTS.md](AGENTS.md) is the full guide: the
architecture, conventions and checks every change must pass. It applies to people as
much as to coding agents.

Everyone taking part follows the [code of conduct](CODE_OF_CONDUCT.md).

## Reporting a bug or asking for a feature

Open an [issue](https://github.com/irori-app/irori/issues). For a bug, include:

- Your OS and version, how you installed Irori, and the Irori version
  (`irori --version`).
- The store and game, if it is about one.
- What you did, what you expected and what happened.
- The relevant part of the log: `data/logs/irori.log` under Irori's data directory
  (see [platform paths](docs/PLATFORMS.md#paths-and-desktop-behavior)), and any
  `crash-*.txt` beside it.

Logs can contain your user name, file paths and game titles. Read them before you post
them, and never post tokens, cookies or account ids.

## Setting up

You need Rust 1.95 or newer and the platform libraries listed in
[docs/PLATFORMS.md](docs/PLATFORMS.md). Then:

```bash
cargo run --release -p irori
```

Use `IRORI_HOME=<scratch dir>` while you experiment, so your real library, settings and
store sign-ins stay untouched.

## Making a change

1. Branch from an up-to-date `main` with a name such as `fix/epic-login` or
   `feat/big-picture-search` (`<type>/<short-kebab-description>`).
2. Keep the change to one topic and match the code around it.
3. Add tests: new behaviour needs them, and a bug fix needs a test that fails
   without the fix. Tests never touch the network or real user data.
4. Add or update user-facing strings in `crates/<crate>/locales/en.json`. Other 18
   locale files fall back cleanly to English at runtime when keys are missing, so
   code PRs do not need machine-translated catalogs; community translators update
   other languages through dedicated `i18n(<lang>)` pull requests or Weblate. When
   translating existing catalogs, keep every `{placeholder}` name exact.
5. Run the checks:

   ```bash
   cargo fmt --all
   cargo clippy --locked --workspace --all-targets --profile ci -- -D warnings
   cargo test --locked --workspace --no-fail-fast
   ```

6. If the change shows up in the app, run Irori and try it. Include screenshots in
   light and dark for UI changes.

## AI-Assisted Contributions

AI coding tools can help draft fixes and tests, provided a human contributor stays accountable for the result:

- Understand and review every line in your diff before opening a pull request.
- Build and test the change manually on real hardware; never submit unverified generated patches.
- Disclose significant AI assistance in the pull request description so reviewers know how the change was produced and tested.
- Keep pull requests small, single-purpose, and reviewable. Pull requests adding more than 1,500 lines across `*.rs` files fail CI unless split into stacked PRs or approved with the `large-pr-approved` label.

## Scope and Non-Goals

Irori focuses on being a fast, calm, native GPUI desktop and Big Picture game launcher for Linux, macOS, and Windows supporting official stores, emulators/ROM libraries, Lutris install scripts/imports, and sideloaded apps.

Out of scope for the repository:

- Parallel UI toolkits or rewrites (such as Electron, Qt, Kirigami, or embedded web engines in the main window).
- Unofficial downloaders, DRM bypasses, or piracy sources.
- Broad third-party plugin engines before core launcher stability and packaging reach maturity.
- Telemetry, analytics SDKs, or automatic crash uploaders.

If you want to propose a large new feature, start a conversation on [GitHub Discussions](https://github.com/irori-app/irori/discussions) before writing code.

## Pull requests

- Title the pull request `<type>(<scope>): <summary>`, with the same type as the
  branch, for example `fix(epic): treat Mac ports released separately as native`.
  The types are `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `i18n`, `build`,
  `ci`, `chore` and `revert`. A CI check enforces naming, the 1,500-line Rust diff
  limit, and the PR body structure (`## Plain-language summary` before
  `## Technical details`); you can run it locally:

  ```bash
  python3 .github/scripts/pr_rules.py --title "fix(epic): ..." --head fix/epic-login --body-file pr_body.md
  ```

- Fill in the pull request template: start with a short plain-language summary of
  what changes for users before the technical details, list how you checked it and
  which platforms you didn't try, and attach before-and-after screenshots in both
  light and dark themes for any UI or UX change.
- Pull requests are squash-merged, with the title as the commit message.

## Licence

Irori is under the [MIT License](LICENSE). By contributing, you agree that your
contribution is licensed under it too.
