# apis — design

Pure, sandboxed `eval`/`build`/`instantiate`/`run`/`repl` over the repo you are
standing in, without flakes. Rust over the Nix C API
(`nixops4/nix-bindings-rust`, pinned by rev), with three deliberate shell-outs.

## Two eval states, one impurity

Pure eval cannot see the working tree and has no `builtins.currentSystem`, so
apis runs one impure step and everything else pure.

1. **Impure.** Realise the source and read `builtins.currentSystem`. The source
   is the working tree (list it with `git`, add it to the store with
   `builtins.path` and a filter) or, with `--ref`, one commit's tree.
2. **Pin it.** A narHash: `nix hash path` over the snapshot, or the `narHash`
   `fetchGit` already reports.
3. **Pure.** A fresh `EvalState` with `pure-eval` on, `self` re-injected as the
   same source locked to that hash — so pure eval accepts it — and the host
   system passed to the entry point.

Flakes do this same on-the-fly hashing to produce `self`. The lock is a policy
guardrail, not a technical limit: a dirty tree has no rev, so it is "unlocked"
until we hash it ourselves.

## What the snapshot contains

One `git ls-files --cached --others --exclude-standard` — tracked ∪ untracked,
gitignored excluded, in a single call. The file set drives `builtins.path`'s
`filter` as a `listToAttrs` lookup, with every ancestor directory added
explicitly: the filter is asked about directories too, and rejecting one prunes
everything beneath it.

**Symlinks into `/nix/store` are dropped.** `./result*` links are untracked and
unignored, and a NAR records a symlink by its target string — so including them
would change the snapshot hash on every build and invalidate `self` against
itself. Widening `.gitignore` to `/result*` would cover the common case; the
skip is the safety net.

## `--ref`: a commit instead of the working tree

`git rev-parse --verify <ref>^{commit}` first — `fetchGit` accepts nothing
shorter than a full sha, and this is where a typo gets a readable error. Then
`builtins.fetchGit { url = <root>; rev; allRefs = true; }`, which exports that
commit's tree (tracked files only, no `.git`, the dirty checkout ignored) and
caches it per rev. `allRefs` so a rev off the checked-out branch still resolves.

**`self` stays a `fetchGit` call under pure eval**, locked by `rev` + `narHash`,
rather than collapsing to `builtins.path { path = <outPath>; }`. Nix 2.35 knows
a fetched store path before it materializes it: `nix eval` prints an `outPath`
that `nix path-info` calls *not valid* until something reads through it. So
`nix hash path` and `builtins.path` both fail on it, while `fetchGit` fetches on
first read. This is the same lock flakes use for a git input.

Old revisions only evaluate if their entry point takes the platform. Anything
predating the `{ system }` entry falls through to
`builtins.currentSystem`, which pure eval does not have, and fails inside
nixpkgs.

## Why the `nix` CLI is still needed

| call | why it can't be in-process |
|---|---|
| `nix hash path` | no `builtins.hashPath`; the C API has no NAR-hash query. Working-tree snapshots only — `fetchGit` reports its own |
| `nix build …^*` | only to stream `--log-format internal-json` into `nom` |
| `nix repl` | the REPL lives in `libcmd`, outside the stable C API |

`Store::realise` *is* exposed, so builds could be in-process — at the cost of
nom's live build tree. The CLI is the price of that output.

## Contracts

- **`run` picks its binary by `meta.mainProgram` first**, then the last
  component of the attrpath, then the sole entry in `bin/`. The attrpath is
  only a guess and is wrong whenever a package's binary is named differently
  (`slippi-dolphin` ships `dolphin-emu` and `dolphin-emu-nogui`).
- **`--expr` is pure.** It widens expressiveness, never reach.
- Tool paths (`APIS_NIX`, `APIS_NOM`, `APIS_GIT`) are baked in at build time via
  `option_env!`, falling back to `PATH` so plain `cargo build` works.

## Constraints that will bite

- **`pure-eval` cannot be set through the C API settings registry** ("Setting
  not found"). Pass it as `NIX_CONFIG=pure-eval=true` in the environment before
  constructing the second `EvalState`, which reads config at creation.
- **Pure eval has no `builtins.currentSystem`, and `eval-system` does not
  restore it.** apis reads the host system during the impure probe and applies
  the entry point with `builtins.intersectAttrs (builtins.functionArgs r)
  { system = …; }`, so a no-arg function or a plain-attrset entry still work.
  One system axis, always the host: apis does not cross-compile and never offers
  `localSystem`/`crossSystem`.
- **`self` is a string, not a path.** `builtins.path` returns a string in Nix.
  `import self` and `self + "/x"` both work, but this differs from flakes, where
  `self` is an attrset with `.outPath`.
- **NixOS configs take their platform off `pkgs`, not off `eval-config`** — pass
  `system = null` plus `nixpkgs.buildPlatform`/`hostPlatform`, never a `system`
  string, or eval-config defaults to `builtins.currentSystem` and dies under pure
  eval. `nixpkgs.pkgs` would conflict with the config's own overlays and
  `allowUnfree` settings. See `pkgs/desktop-system-applicator/default.nix`.
- **`apis` links libnixexpr from `nixVersions.latest`**, so a nixpkgs bump that
  moves the C API can break this package first. `nixVersions.nix_2_35` pins it.
- **Known unfixed:** `desktop-iso` fails under pure eval only — grub's
  `import-efisetjmp.patch` reported "not valid". It builds impurely.
