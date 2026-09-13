# seatmux — intent

- Split one machine into separate seats that share a GPU. My desk (two
  monitors, keyboard, mouse) and the TV (its own keyboard, mouse, sound), each
  running its own Wayland compositor.
- One login, one GPU. Games render and scan out on the desk card for either
  seat — no second GPU, no render offload, no per-frame copies.
- Neither seat is privileged. Either compositor restarts without disturbing the
  other.
- Each seat gets its own audio sink and source, decided when it starts.
- Each seat gets its own environment too — timezone, theme, editor. Same on both
  for now, but they should be able to differ.
- Each seat gets its own cgroup, so a runaway game on the TV cannot starve or
  OOM the desk. seatmux makes them itself — it has to fork the compositors
  directly for the DRM lease to be inherited, so it owns their placement too.
- Rootless. No udev rules, no group changes, nothing in the system config.
- Start with two sways. A dedicated TV kiosk — gamescope, cage — can come later
  without changing the shape.
- `seatmux start` to bring the seats up, `seatmux status` to see what they are
  doing, `seatmux stop` to take everything down in one go. Nothing starts from a
  bare `seatmux`.
