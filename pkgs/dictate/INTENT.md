# dictate — intent

Distilled intent for this package — bullets, no fluff, the minimum to reproduce
it. The *why* and *what we want*; mechanism is in the code. Reconcile over time.

## Problem

- Hold-to-talk dictation: hold a key, speak, words type into the focused window; release ends it.
- Text must appear *while you're still talking* — not batched on release, not gated on pauses.

## Approach

- Keep the entire clip (everything since key-down); never chunk it, never trim it.
- In a loop, re-decode the whole clip and reconcile the field toward that decode via a multi-region character diff — edit only the spots that changed, navigating the unchanged text by cursor, so revising an early word doesn't rewrite the rest of the line.
- Before each reconcile the typist re-anchors the cursor to the end of the field (End key). The typist models the field, but a dropped/stray key would desync that model from reality and make the next edit delete the wrong span (the "text randomly disappears" bug). Re-anchoring every time means drift can never compound across reconciles.
- Re-decoding the *whole* clip means the model always has full context, so it never hallucinates filler or drops words. The field converges on the full-clip decode; the settled text *is* exactly the offline full-file decode, by construction.
- Run the model back-to-back (no fixed interval) — the decode time is the natural pace. For a typical ~15 s hold it's fast enough to track speech live.

## Structure (actors)

- Three decoupled threads passing events (not async): capture (pw-record's raw-PCM stdout → samples appended to a shared clip), the decoder (loop: whole clip → text, sent on change), and the typist (latest desired text → minimal field edits). The decoder never blocks on typing and vice-versa.
- Audio is a raw-PCM stdout pipe from `pw-record --raw -` straight into the capture thread — no WAV file, no header-seek, no polling. (`record`/`replay` still use a WAV file for offline test clips.)
- The typist coalesces to the *latest* decode (skips stale intermediates) and decodes are back-to-back, so the field always converges toward the freshest full-clip decode.
- Typing is a persistent Wayland `virtual-keyboard` client (one connection, our own XKB keymap) — no process spawn per edit, and it can interleave cursor moves with text for the multi-region edits. Replaces per-edit `wtype`.

## Properties we want

- Streaming while talking — the field updates as each decode lands, and stabilizes on the full clip.
- Correct by construction — the settled text is exactly the full-file decode: no dropped words, no hallucinated filler, no duplicates.
- Fast and efficient — model stays resident (loaded once per hold). Re-decoding the whole clip is O(clip length) per pass: fine for short holds, grows with length (no cap yet).
- Parakeet (NVIDIA), offline, for accuracy + built-in punctuation/capitalization.
- Transient flicker as recent words revise with more context is expected; only the settled text must be right.

## Rejected

- Chunking / silence-segmenting the audio — tried many variants; every one introduced boundary artifacts (dropped chunks, hallucinated "Um/Mm" filler, duplicated words), because the model garbles partial/quiet audio. Re-decoding the whole clip sidesteps all of it: the model only ever sees full context.
- A natively-streaming model in place of Parakeet — loses accuracy/punctuation.

## Verify

- The reconcile is a pure function (`typed` + desired text → cursor ops); the mic and keyboard are at the very edges, so the exact reconcile runs without them.
- `dictate record <wav>` captures a clip; `dictate replay <wav>` streams it through the reconcile path and word-diffs the result against a full-file decode — dropped = words lost, inserted = filler hallucinated. Both are zero for the full-reprocess approach.
