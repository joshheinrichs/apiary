# Get the icon for an app by searching .desktop files
get_icon_name() {
    local app_id="$1"
    for d in ${XDG_DATA_DIRS:-/run/current-system/sw/share} "$HOME/.local/share"; do
        [ -d "$d/applications" ] || continue
        local desktop
        desktop=$(find "$d/applications" -type f -name '*.desktop' 2>/dev/null \
            | xargs grep -ilE "^(Name|StartupWMClass)=${app_id}$" 2>/dev/null \
            | head -n1) || continue
        [ -n "$desktop" ] && grep -m1 "^Icon=" "$desktop" | cut -d= -f2 && return 0
    done
}

# Get list of windows
windows=$(swaymsg -t get_tree | jq -r '
def walk_tree: recurse(.nodes[]?, .floating_nodes[]?);

# Normal windows
(walk_tree
    | select(.type == "con" and .name != null)
    | "\(.id)\t\((.app_id // .window_properties.class // "unknown"))\t\(.name)") ,

# Scratchpad windows
(walk_tree
    | select(.type == "workspace" and .name == "__i3_scratch")
    | .floating_nodes[]?
    | "\(.id)\t\((.app_id // .window_properties.class // "unknown"))\t\(.name) [scratchpad]")
')

[ -z "$windows" ] && exit 0

# Build list with icons
list=""
while IFS=$'\t' read -r id app title; do
    icon=$(get_icon_name "$app" || true)
    [ -n "$icon" ] && icon="icon:$icon\t"
    list+="${id}\t${icon}${app} - ${title}\n"
done <<< "$windows"

# Ask user to choose a window
chosen=$(echo -e "$list" | cut -f2- | fuzzel --dmenu --prompt "Window: ")
[ -z "$chosen" ] && exit 0

# Focus or show scratchpad
win_id=$(echo -e "$list" | grep -F "$chosen" | cut -f1)
swaymsg "[con_id=$win_id]" scratchpad show || swaymsg "[con_id=$win_id]" focus
