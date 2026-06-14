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
  # Hold-to-talk dictation: `dictate start` loads the model and records the mic,
  # re-decoding the whole clip back-to-back and reconciling the focused window's
  # text toward each decode via the virtual-keyboard protocol as you speak.
  # `dictate stop` does the final full-clip decode. Wired to press/release sway
  # keybindings.
  #
  # sherpa-onnx is linked in-process so the model is loaded once per hold; all
  # dependency paths are substituted directly into the source at build time.
  pkgs.rustPlatform.buildRustPackage {
    pname = "dictate";
    version = "0.1.0";
    src = ./.;
    cargoLock.lockFile = ./Cargo.lock;
    nativeBuildInputs = [ pkgs.rustPlatform.bindgenHook ];
    buildInputs = [ pkgs.sherpa-onnx ];
    postPatch = ''
      substituteInPlace build.rs \
        --replace-fail '@sherpa@' '${pkgs.sherpa-onnx}'
      substituteInPlace src/main.rs \
        --replace-fail '@pipewire@' '${pkgs.pipewire}' \
        --replace-fail '@model@' '${model}' \
        --replace-fail '@target@' '${target}'
    '';
  }
) { }
