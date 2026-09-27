# apiary

under construction

## usage

bootstrap apis

```bash
nix-build -A apis
```

```bash
apis run mainframe-system-applicator
```

`mainframe-home-applicator` puts `apis` on `PATH`, so after the first run the
`./result/bin/` prefix is no longer needed.

## goals

* build-oriented
* minimal system-level configuration
* sandbox applications
* proper cgroup hierarchy
* continuous profiling
