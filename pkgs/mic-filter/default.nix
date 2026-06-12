{ pkgs }:
pkgs.lib.makeOverridable (
  {
    # Linear gain applied after noise suppression; 2.0 = +6 dB.
    gain ? 2.0,
    # Maximum noise attenuation in dB; lower it to let some ambience through.
    attenuationLimit ? 100,
  }:
  let
    # A standalone pipewire process hosting a filter-chain: it captures the
    # default source, runs DeepFilterNet noise suppression and a gain stage,
    # and exposes the result as a virtual source ("DeepFilter Microphone")
    # that apps select like a normal mic.
    conf = pkgs.writeText "mic-filter.conf" ''
      context.properties = {
        # errors + warnings; anything lower hides load failures from the journal
        log.level = 2
      }
      context.spa-libs = {
        audio.convert.* = audioconvert/libspa-audioconvert
        support.*       = support/libspa-support
      }
      context.modules = [
        {
          name = libpipewire-module-rt
          args = {
            nice.level = -11
          }
          flags = [ ifexists nofail ]
        }
        { name = libpipewire-module-protocol-native }
        { name = libpipewire-module-client-node }
        { name = libpipewire-module-adapter }
        {
          name = libpipewire-module-filter-chain
          args = {
            node.description = "DeepFilter Microphone"
            media.name = "DeepFilter Microphone"
            filter.graph = {
              nodes = [
                {
                  type = ladspa
                  name = df
                  plugin = ${pkgs.deepfilternet}/lib/ladspa/libdeep_filter_ladspa.so
                  label = deep_filter_mono
                  control = {
                    "Attenuation Limit (dB)" = ${toString attenuationLimit}
                  }
                }
                {
                  type = builtin
                  name = boost
                  label = mixer
                  control = {
                    "Gain 1" = ${toString gain}
                  }
                }
              ]
              links = [
                { output = "df:Audio Out" input = "boost:In 1" }
              ]
              inputs = [ "df:Audio In" ]
              outputs = [ "boost:Out" ]
            }
            # DeepFilterNet only operates at 48kHz
            audio.rate = 48000
            audio.position = [ MONO ]
            capture.props = {
              node.name = "capture.deepfilter_mic"
              node.passive = true
            }
            playback.props = {
              node.name = "deepfilter_mic"
              media.class = Audio/Source
            }
          }
        }
      ]
    '';
  in
  # pipewire only dlopens LADSPA plugins found under LADSPA_PATH (it joins
  # search dirs with the plugin name rather than accepting arbitrary
  # absolute paths), so the plugin's lib dir must be on the path.
  pkgs.writeShellScriptBin "mic-filter" ''
    export LADSPA_PATH=${pkgs.deepfilternet}/lib/ladspa
    exec ${pkgs.pipewire}/bin/pipewire -c ${conf}
  ''
) { }
