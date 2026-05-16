# scoper

Wraps a command in a transient systemd user scope, placed within a derived slice hierarchy.

## CLI

```
scoper --slice=<name> --name=<unit-name> -- <cmd> [args...]
```

- `--slice`: slice name to create/use, relative to the detected parent slice
- `--name`: scope unit name (no `.scope` suffix needed)

## Slice detection

Read `/proc/self/cgroup` (unified hierarchy, `0::` line). Strip the `user.slice/user-NNN.slice/user@NNN.service/` prefix to get the user-manager-relative path. If that prefix isn't present (e.g. raw logind TTY session), treat the user manager root as the base (empty prefix).

Strip the trailing unit (last path component) to get the parent slice. Append `--slice` to it to get the target slice.

## Example tree

```
user@1000.service/
  session-3.slice/          # scoper --slice=session-3 --name=sway -- sway
    sway.scope
    apps.slice/             # scoper --slice=apps --name=app-discord-PID -- discord
      app-discord-PID.scope
      app-foot-PID.scope
```

## Integration

- `wm` launches sway via: `scoper --slice=session-$(systemd-escape "$XDG_SESSION_ID") --name=sway -- sway`
- sway config uses scoper as launch prefix for fuzzel and terminal (already done)
