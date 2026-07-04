let
  data = builtins.fromJSON (builtins.readFile ./devices.json);
in
{
  inherit data;
  monitorById =
    id:
    let
      matches = builtins.filter (m: m.serial == id) data.monitors;
    in
    if matches == [ ] then throw "monitor ${id} not in desktop-devices" else builtins.head matches;
}
