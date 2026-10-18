# firefox — intent

- nixpkgs Firefox with VA-API frame pool fixes until they ship upstream.
- Our own: unlinked surfaces become Retired and are dropped once idle, so the
  pool (and its eventfds) stops growing during playback. Fix the pool's data
  model, not sweep up after it. Upstream it; drop each patch once a release
  has it.
