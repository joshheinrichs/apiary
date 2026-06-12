{ pkgs }:
pkgs.lib.makeOverridable (
  {
    # PipeWire node to record from; falls back to the default source if the
    # node isn't present (e.g. mic-filter isn't running).
    target ? "deepfilter_mic",
  }:
  let
    # NVIDIA Parakeet-TDT 0.6B v2 (English), int8 ONNX export from the
    # sherpa-onnx project (sherpa-onnx is the runtime, Parakeet the model).
    # Punctuation and capitalization are part of the model output.
    model = pkgs.fetchzip {
      url = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8.tar.bz2";
      sha256 = "12xwj3qp44l1m2ydfg4acbn5a13k6filisycgc5nnl6zq8vxp3ak";
    };
  in
  # Hold-to-talk dictation: `dictate start` records the mic, `dictate stop`
  # transcribes the recording and types the text into the focused window via
  # the virtual-keyboard protocol. Wired to press/release sway keybindings.
  pkgs.rustPlatform.buildRustPackage {
    pname = "dictate";
    version = "0.1.0";
    src = ./.;
    cargoLock.lockFile = ./Cargo.lock;
    env = {
      DICTATE_PIPEWIRE = "${pkgs.pipewire}";
      DICTATE_SHERPA_ONNX = "${pkgs.sherpa-onnx}";
      DICTATE_WTYPE = "${pkgs.wtype}";
      DICTATE_MODEL = "${model}";
      DICTATE_TARGET = target;
    };
  }
) { }
