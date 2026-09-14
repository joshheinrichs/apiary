let
  data = builtins.fromJSON (builtins.readFile ./devices.json);
  byAttr =
    attr: list: id:
    let
      matches = builtins.filter (x: x.${attr} == id) list;
    in
    if matches == [ ] then throw "device ${id} not in mainframe-devices" else builtins.head matches;
  byId = byAttr "serial";
in
{
  inherit data;
  monitorById = byId data.monitors;
  # For monitors without an EDID serial (TVs).
  monitorByModel = byAttr "model" data.monitors;
  audioById = byId data.audio;
  inputByName = byAttr "name" data.inputs;
  # Raw USB devices, keyed by product name (e.g. "WUP-028" for the GameCube
  # adapter). The device's identifier carries the udev-ready hex "vid:pid:serial".
  usbByName = byAttr "name" data.usb;
}
