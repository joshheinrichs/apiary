# firefox — intent

- nixpkgs Firefox with VA-API frame pool fixes until they ship upstream.
- Bug 2067504 backported from mozilla-central: one poll() for the whole pool
  instead of one per surface per frame.
- Our own: unlinked surfaces become Retired and are dropped once idle, so the
  pool (and its eventfds) stops growing during playback. Fix the pool's data
  model, not sweep up after it. Upstream it; drop each patch once a release
  has it.
