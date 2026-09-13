# fm1ctl

- I want to flash my FM-1's firmware from Linux. The vendor only ships a
  Windows and macOS updater.
- Rust, native. Don't drag Wine into it.
- Start read-only: prove the protocol works before anything writes to flash.
- A Chrome page over Web MIDI is interesting as a follow-on -- it would work for
  anyone on any OS, and Web MIDI handles the device dropping off and coming
  back mid-flash. Undecided whether the actual write should happen in a browser
  tab.
- CLI grouped as noun then verb: `fm1ctl firmware fetch`, `fm1ctl firmware
  flash`. Keep multiple firmware versions around rather than just the latest,
  and tell me to fetch if I have none.
- Nice to have: read the presets out too, not just firmware.
