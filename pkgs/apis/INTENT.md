# apis

- Pure eval, without flakes. No `flake.nix`, no inputs schema, no lock file —
  just the good parts: the working tree as source, and a sandboxed eval scoped
  to the folder I'm standing in.
- The repo is a function library. apis is one more entry point over it, not a
  framework around it.
- Name: Latin for "bee", the `api-` root in apiary — and a pun, since the repo
  is meant to be consumed as a library.

## What it can see

- It has to work in a dirty repo. The working tree is the source: tracked and
  untracked files alike, exactly as they are on disk right now.
- Gitignored files are the only thing filtered out. That's the rule — if git
  ignores it, apis doesn't see it.
- Symlinks into `/nix/store` stay out too, `./result*` included. Otherwise a
  build repoints one and the snapshot invalidates itself.
- apis can only ever see the folder I'm in. That's the invariant.
- `--ref` to run a version that isn't in the checkout — a branch, tag or sha
  instead of the working tree. Still the same folder, just an older tree.
- `--expr` widens what I can *say*, not what apis can *reach* — it stays pure.
  An impure escape would have to be its own explicit flag.

## Shape

- A swap-in for `nix`: `eval`, `build`, `run`, `instantiate`, `repl`. That set
  is enough — I don't need the rest of the `nix` surface.
- `develop` would be cool, but it's lower priority than the five above.
- A positional argument is an attrpath into the repo; `--expr` is a raw
  expression with `self` in scope. No separate `expr` subcommand.
- `repl` drops into a Nix REPL with the repo attrs and `self` in scope,
  sandboxed.
- No cross-compilation. One system axis, always the host I'm on.
- Builds should look alive: nom's build tree, not a wall of log lines.
- Bake absolute tool paths in at build time. No PATH wrapper.

## Ideas, not built

- A native REPL over the Nix C API with a persistent EvalState, instead of
  shelling out to `nix repl`.
- An explicit `--impure` flag, if a real need shows up.
