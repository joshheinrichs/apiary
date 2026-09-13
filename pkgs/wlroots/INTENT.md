# wlroots — intent

- Patched wlroots so a compositor can be handed a DRM lease instead of opening
  a GPU itself. Wanted so sway and a TV kiosk run as siblings under a broker
  that owns the card — one GPU, both scanning out directly.
- Neither compositor is privileged. Either should restart without disturbing
  the other; that is why the broker holds master rather than sway.
- sway should need no patch of its own — just build it against this.
- Keep the fork small and upstreamable: consuming a lease is the missing
  counterpart to the lease-*offering* support already in wlroots.
