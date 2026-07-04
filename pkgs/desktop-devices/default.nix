let
  data = builtins.fromJSON (builtins.readFile ./devices.json);
  byId =
    list: id:
    let
      matches = builtins.filter (x: x.serial == id) list;
    in
    if matches == [ ] then throw "device ${id} not in desktop-devices" else builtins.head matches;
in
{
  inherit data;
  monitorById = byId data.monitors;
  audioById = byId data.audio;
}
