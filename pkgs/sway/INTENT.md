# sway — intent

- Stock sway, no patches of its own. The only change is building it against
  our wlroots, so it can be handed a DRM lease instead of opening a GPU
  itself.
- Two instances: the desk and the TV kiosk, siblings under a broker that owns
  the card. Same binary, different config, different lease.
- The old seat patches (per-seat map focus, focus fence, bindings none) are
  dropped. They belonged to the in-compositor multi-seat attempt, which the
  lease design replaces — the split now lives below sway, in DRM.
