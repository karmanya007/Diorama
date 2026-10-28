# Contributing to Diorama

Thanks for helping. Bug reports, compatibility reports and pull requests are all welcome.

## Reporting a wallpaper that looks or behaves wrong

Open an issue with:

- the Workshop id (or the Wallpaper Engine default project's name);
- what differs from Wallpaper Engine, ideally with screenshots of both side by side;
- `diorama status` output, and the relevant part of `%APPDATA%\Diorama\dioramad.log` (plus `diorama-webhost.log` for web wallpapers).

Do **not** attach the wallpaper's files or anything extracted from them. Workshop content belongs to its authors; the id is enough to find it.

## Development setup

See [Building from source](README.md#building-from-source). The architecture and the reasons behind it are in [docs/adr](docs/adr); read ADR-002 (scene format and layering) before larger changes.

## Before you open a pull request

All of these must pass; CI runs the same checks:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
bash scripts/check-layers.sh     # architecture rules
cargo deny check                 # licences, advisories, sources
```

For changes to the Diorama window, also run `npm run build` in `apps/diorama-ui`.

Guidelines:

- **Layering**:
  - lower crates never depend on higher ones;
  - the renderer never sees Wallpaper Engine formats;
  - OS-specific code stays in `diorama-platform` and `diorama-media`.

  `scripts/check-layers.sh` enforces this.
- **Workshop content is untrusted input.** Parsers return errors instead of panicking (`unwrap`/`expect` are denied in library crates), check sizes and offsets, and cap allocations. Add a fuzz target or a test with hostile input when you touch a parser.
- **No Wallpaper Engine code or content in the repository.** That covers:
  - no copied shaders, scripts, assets or binaries;
  - no code ported from decompiled output.

  Describe behaviour in your own words in `docs/re-notes` and implement it from that description. Tests that need real content read it from the local installation and skip themselves when it is missing.
- **Dependencies** must be under licences that `deny.toml` allows; copyleft licences (GPL, AGPL) are not accepted.
- Keep commits focused, with messages that say what changed and why ([Conventional Commits](https://www.conventionalcommits.org) style: `fix(render): …`).

## Licence

By contributing you agree that your work is dual-licensed under MIT or Apache-2.0, like the rest of Diorama, without additional terms.
