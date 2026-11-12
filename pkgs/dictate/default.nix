{ pkgs }:
pkgs.lib.makeOverridable (
  {
    # PipeWire node to record from; falls back to the default source if the
    # node isn't present (e.g. mic-filter isn't running).
    target ? "mic-filter",
  }:
  let
    # NVIDIA Nemotron Speech Streaming EN 0.6B, 560 ms chunks, int8, as
    # sherpa-onnx exports it. Punctuation and capitalization are in the output.
    model = pkgs.fetchzip {
      url = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemotron-speech-streaming-en-0.6b-560ms-int8-2026-04-25.tar.bz2";
      sha256 = "1mszhv9v1wifkarcy3278ahx9zh3i3h5v9ngnwqapk0chg3cc845";
    };
  in
  # Hold-to-talk dictation: `dictate start` records the mic into one streaming
  # decode and types each new piece of the transcript with wtype as it lands;
  # `dictate stop` hears out the last chunk. Wired to press/release sway
  # keybindings.
  pkgs.rustPlatform.buildRustPackage {
    pname = "dictate";
    version = "0.1.0";
    src = ./.;
    cargoLock.lockFile = ./Cargo.lock;
    nativeBuildInputs = [ pkgs.rustPlatform.bindgenHook ];
    buildInputs = [ pkgs.sherpa-onnx ];
    env = {
      DICTATE_SHERPA = "${pkgs.sherpa-onnx}";
      DICTATE_MODEL = "${model}";
      DICTATE_PW_RECORD = "${pkgs.pipewire}/bin/pw-record";
      DICTATE_PW_CLI = "${pkgs.pipewire}/bin/pw-cli";
      DICTATE_WTYPE = "${pkgs.wtype}/bin/wtype";
      DICTATE_TARGET = target;
    };
    # A sentence said by espeak-ng, and a minute of it, to replay in the sandbox.
    nativeCheckInputs = [
      pkgs.espeak-ng
      pkgs.sox
    ];
    preCheck = ''
      espeak-ng -s 150 -w said.wav "The agents finished the build, and all the tests passed."
      sox said.wav -r 16000 -c 1 -b 16 fixture.wav
      sox fixture.wav long.wav repeat 19
      export DICTATE_FIXTURE=$PWD/fixture.wav DICTATE_LONG_FIXTURE=$PWD/long.wav
    '';
  }
) { }
