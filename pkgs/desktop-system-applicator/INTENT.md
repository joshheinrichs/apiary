# INTENT

- Run a photo library on the desktop — browsing, albums, faces, object search —
  reachable from the public internet so links can be shared with other people.
- Immich is the thing to run for this.
- Publish it through a Cloudflare Tunnel rather than forwarding ports: nothing
  is exposed on the router, and it survives wgnord holding the default route.
  Accepting the free tier's 100MB per-request cap, which limits large video
  uploads.
- Serve it from our own domain, not a generated hostname.
