# winry315 — intent

Macropad to tinker with. 15 keys, 3 knobs, RGB LEDs. Prototyping, nothing
precious.

- Modal. Knobs do different things per mode.
- Bottom row picks the mode. One key per mode, direct, no cycling.
- LEDs show which mode I'm in.
- Easy to add to. New idea shouldn't mean reflashing.
- LEDs are desired state, not events. Anything that wants to affect them says
  what it wants; one thing decides what the pad actually shows.
- Slow inputs must never hold up the pad. Album art comes off the network and
  arrives whenever it arrives — the lights carry on without it and pick it up
  when it lands.
- Newest wins, always. Never queue frames: a late frame is a wrong frame.
- Colours ease between states; levels do not. Anything driven by audio lands
  immediately.

## Modes, priority order

- **Colour** *(built)* — knobs are R/G/B, pad shows the colour.
- **Mouse** — left knob X, right knob Y, middle scrolls, knob clicks are buttons.
- **Monitor brightness** — on a knob.
- **Spotify** *(built)* — knobs drive Spotify, pad shows the album flashing
  along to it. See below.
- **Volume** — global, on a knob.

## Spotify mode

- Middle knob clicks play/pause and turns Spotify's own volume, not the
  system's. Left and right knobs seek when turned; clicking them jumps
  back/forward.
- The album fills the grid. It's a grid of LEDs, so use it — the cover
  downsampled across the pad, not one flat wash.
- Pull the colours out properly. A downsampled cover should read as the album's
  colours, not as mud.
- Ease between albums. A new cover shouldn't snap into place.
- The volume multiplier is the exception: that lands immediately, no easing.

## Sound on the pad

Colour comes from the album art, motion from the audio. Tap Spotify itself, not
whatever the system happens to be playing.

- The grid is 5x4. The knob LEDs are its top row, not a separate strip — they
  sit right above the keys, so they're part of the picture.
- Frequency runs up the grid and stereo runs across it: low at the bottom, left
  channel on the left. Panning moves the light across the pad.
- Four rows, four frequency buckets, sized the way hearing works — by ratio, so
  each covers the same span of octaves rather than the same number of hertz.
- Blend across the buckets rather than dropping each sound into one. A rising
  tone should slide up the pad, not jump between rows.
- Hits land immediately and fade out gradually. Rising and falling at the same
  speed reads as flicker rather than a pulse.
- The underglow is a column of three down each side: the left column is the left
  channel, the right column the right. They *are* the channels, so nothing about
  panning applies to them.
