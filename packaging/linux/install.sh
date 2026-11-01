#!/bin/sh
# Installs Bit for the current user: the program in ~/.local/bin, its icon in
# the icon theme, and a menu entry. Nothing needs root.
#
#   ./install.sh              install
#   ./install.sh --autostart  also start Bit when you log in
#   ./install.sh --software   start Bit with software drawing (for old graphics
#                             chips that show a black settings window)
#   ./install.sh --uninstall  remove everything this script added
#
# Options can be combined. It works from an unpacked release download, or
# from a source checkout after `cargo build --release` (packaging/linux/).
set -e
here=$(cd "$(dirname "$0")" && pwd)
bin="$HOME/.local/bin"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
autostart="${XDG_CONFIG_HOME:-$HOME/.config}/autostart"
sizes="16 32 48 64 128 256 512"

uninstall=no
login=no
software=no
for arg in "$@"; do
    case "$arg" in
        --uninstall) uninstall=yes ;;
        --autostart) login=yes ;;
        --software) software=yes ;;
        *) echo "Unknown option: $arg" >&2; exit 1 ;;
    esac
done

if [ "$uninstall" = yes ]; then
    rm -f "$bin/tron-bit" "$data/applications/tron-bit.desktop" "$autostart/tron-bit.desktop"
    for s in $sizes; do rm -f "$data/icons/hicolor/${s}x${s}/apps/tron-bit.png"; done
    echo "Bit removed. (Its settings stay in ~/.config/tron-bit.)"
    exit 0
fi

# The release download keeps everything side by side; a source checkout
# keeps the program in target/release and the icons in assets/icon.
program="$here/tron-bit"
icons="$here/icons"
if [ ! -f "$program" ]; then
    program="$here/../../target/release/tron-bit"
    icons="$here/../../assets/icon"
fi
if [ ! -f "$program" ]; then
    echo "Can't find the tron-bit program. Build it first with: cargo build --release" >&2
    exit 1
fi

mkdir -p "$bin" "$data/applications"
install -m 755 "$program" "$bin/tron-bit"
for s in $sizes; do
    mkdir -p "$data/icons/hicolor/${s}x${s}/apps"
    install -m 644 "$icons/bit-$s.png" "$data/icons/hicolor/${s}x${s}/apps/tron-bit.png"
done
exec_line="$bin/tron-bit"
if [ "$software" = yes ]; then
    exec_line="env LIBGL_ALWAYS_SOFTWARE=1 $bin/tron-bit"
fi
# The icon by its full path: some menus don't pick up a name from the icon
# theme until the desktop restarts, or at all.
sed -e "s|^Exec=.*|Exec=$exec_line|"     -e "s|^Icon=.*|Icon=$data/icons/hicolor/256x256/apps/tron-bit.png|"     "$here/tron-bit.desktop" > "$data/applications/tron-bit.desktop"
if [ "$login" = yes ]; then
    mkdir -p "$autostart"
    cp "$data/applications/tron-bit.desktop" "$autostart/tron-bit.desktop"
fi
command -v update-desktop-database >/dev/null && update-desktop-database "$data/applications" 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q "$data/icons/hicolor" 2>/dev/null || true
echo "Bit installed: find it in your app menu, or run $bin/tron-bit"
