# dictate — intent

- Hold-to-talk dictation: hold a key, speak, words type into the focused window; release ends it.
- Text must appear *while you're still talking* — not batched on release, not gated on pauses.
- Long dictation must stay live: cost per word can't grow with how long you've been talking.
- Nemotron (NVIDIA), streaming, with its own punctuation and capitalization.
- Streaming only appends, so it just types each new piece — no diffing, no cursor edits, no reconciling the field.

## Rejected

- Re-decoding the whole clip with an offline model (Parakeet) and reconciling the field toward each decode — accurate, but slowed down as the clip grew.
- Chunking / silence-segmenting audio for an offline model — boundary artifacts (dropped chunks, hallucinated filler, duplicated words).
