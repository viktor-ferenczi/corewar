#!/usr/bin/env bash
# Record a MARS battle as an MP4 video plus PNG screenshots into media/.
#
# Nothing shows up on the desktop: Weston runs headless with Xwayland, Xephyr runs inside that
# (rootless Xwayland has no root window content to grab), DOSBox runs inside Xephyr and ffmpeg
# grabs Xephyr's screen. 1600x1200 is 320x200 scaled 5x6, the original 4:3 aspect with even pixels.
#
# Usage: [NAME=file_stem] ./record.sh FIRST SECOND SECONDS [mars.py watch options...]
# Example: ./record.sh MICE.CWR KILLER.CWR 90 --delay 20
set -euo pipefail

first=$1 second=$2 seconds=$3
shift 3
here=$(cd "$(dirname "$0")" && pwd)
name=${NAME:-"${first%.CWR}_vs_${second%.CWR}"}
video="$here/media/$name.mp4"
size=1600x1200
mkdir -p "$here/media"

runtime=$(mktemp -d)
pids=()
# Stopping the X servers takes the DOSBox connected to them down as well.
trap 'kill "${pids[@]}" 2>/dev/null; wait 2>/dev/null; rm -rf "$runtime"' EXIT

env -u DISPLAY -u WAYLAND_DISPLAY XDG_RUNTIME_DIR="$runtime" \
    weston --backend=headless --renderer=pixman --xwayland --socket=mars-record \
    --width=1700 --height=1300 --log="$runtime/weston.log" 2>/dev/null &
pids+=($!)
until host=$(grep -o 'xserver listening on display :[0-9]*' "$runtime/weston.log" 2>/dev/null | grep -o ':[0-9]*'); do
    sleep 0.2
done

screen=50
while [ -e "/tmp/.X11-unix/X$screen" ]; do screen=$((screen + 1)); done
DISPLAY=$host Xephyr ":$screen" -screen "$size" -nolisten tcp 2>/dev/null &
pids+=($!)
until xwininfo -display ":$screen" -root >/dev/null 2>&1; do sleep 0.2; done

DISPLAY=:$screen SDL_VIDEO_WINDOW_POS=0,0 python3 "$here/mars.py" watch "$first" "$second" --window "$size" "$@" &
pids+=($!)

# Grab losslessly first, so the screenshots keep the exact VGA colors, then encode a compatible MP4.
lossless="$runtime/capture.mkv"
ffmpeg -hide_banner -loglevel error -y -f x11grab -draw_mouse 0 -framerate 30 -video_size "$size" -t "$seconds" \
    -i ":$screen+0,0" -c:v libx264rgb -qp 0 -preset ultrafast "$lossless"

# Four screenshots spread over the recording.
for percent in 5 25 50 95; do
    ffmpeg -hide_banner -loglevel error -y -ss "$(bc -l <<<"$seconds * $percent / 100")" -i "$lossless" \
        -frames:v 1 "$here/media/${name}_${percent}.png"
done
ffmpeg -hide_banner -loglevel error -y -i "$lossless" -c:v libx264 -pix_fmt yuv420p -crf 20 "$video"
echo "$video"
