# apiary

under construction

## usage

bootstrap anix

```bash
nix-build -A anix
```

```bash
anix run mainframe-system-applicator
```

`mainframe-home-applicator` puts `anix` on `PATH`, so after the first run the
`./result/bin/` prefix is no longer needed.

## goals

* build-oriented
* minimal system-level configuration
* sandbox applications
* proper cgroup hierarchy
* continuous profiling
