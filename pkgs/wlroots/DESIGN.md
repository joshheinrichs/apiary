# wlroots — design

`WLR_DRM_LEASE_FD=<fd>` makes `attempt_drm_backend()` adopt that already-open
fd as its only GPU and return, instead of enumerating.

- `wlr_session_adopt_fd()` (session.c) wraps a foreign fd in a `wlr_device`:
  `fstat` for `st_rdev`, signals initialised, spliced into `session->devices`,
  `device_id = -1`. `wlr_session_close_file()` skips `libseat_close_device()`
  for that sentinel.
- Enumeration is skipped entirely. Without it a lessee calls
  `wlr_session_find_gpus()` and opens every card — sway normally holds DRM
  master on *all* GPUs — and would fight the lessor for the leased one.
- `drm_backend_monitor_create()` is skipped too: a lessee must not adopt GPUs
  that appear later.
- The session is left intact. Only the GPU source changes; libseat still opens
  `/dev/input/event*`, which matters because those nodes are `root:input 0660`
  with no ACL, so a compositor cannot open them directly.

Things that cost time to find:

- wlroots never calls `drmSetMaster`. The only master call in the backend is a
  `drmDropMaster` inside `wlr_drm_backend_get_non_master_fd()`; logind takes
  master when it opens the device. There is no SetMaster path to suppress.
- `wlr_drm_backend_create()` derives `drm->name` from
  `drmGetDeviceNameFromFd2(dev->fd)`, which resolves to the underlying node on
  a lease fd — so the client-fd path needs no special casing.
- sway needs no source patch. It calls `wlr_backend_autocreate()` and uses what
  it gets: `sway-unwrapped.override { wlroots_0_20 = <this>; }`.

Untested: this compiles and sway links against it, but it has never run.
`check_drm_features()` and `init_drm_resources()` do their first real
enumeration on the lease fd at runtime.
