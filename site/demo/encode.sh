#!/usr/bin/env bash
# Encode raw recordings (site/demo/out/<slug>.webm) into the site assets:
# <slug>.mp4 (H.264), .webp (animated, 800 px), .poster.webp,
# and, with GIF=1, a .gif for the hero and the README cards. Budgets per CLIPS.md.
#
#   bash site/demo/encode.sh [slug...]
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
raw="$here/out"
dest="$(cd "$here/.." && pwd)/public/media"
mkdir -p "$dest"

GIF_SLUGS="hero live-session permissions spawn subagents mobile usage"
declare -A TRIM=([hero]="0.8" [live-session]="1.5" [permissions]="1.5" [spawn]="1.0" [subagents]="1.0" [usage]="1.2" [search]="1.2" [themes]="1.0" [mobile]="1.0" [harnesses]="1.2" [rooms]="1.2" [workers]="1.0" [plugins]="1.0" [fleet]="1.0" [voice]="1.5")
declare -A POSTER=([hero]="9" [live-session]="12" [permissions]="7" [spawn]="12" [subagents]="20" [usage]="3" [search]="4" [themes]="6" [mobile]="6" [harnesses]="8" [rooms]="9" [workers]="7" [plugins]="2" [fleet]="6" [voice]="10")

slugs=("$@")
if [[ ${#slugs[@]} -eq 0 ]]; then
	for f in "$raw"/*.webm; do slugs+=("$(basename "$f" .webm)"); done
fi

size_of() { stat -c %s "$1"; }
mb() { awk -v b="$1" 'BEGIN{printf "%.2f", b/1048576}'; }

encode_mp4() {
	local in="$1" out="$2" crf="$3"
	ffmpeg -loglevel error -y -ss "$trim" -i "$in" -an -vf "${pts}scale='min(1280,iw)':-2,format=yuv420p" -t "$len" \
		-c:v libx264 -preset slow -crf "$crf" -movflags +faststart -r 24 "$out"
}

encode_webm() {
	local in="$1" out="$2" crf="$3"
	ffmpeg -loglevel error -y -ss "$trim" -i "$in" -an -vf "${pts}scale='min(1280,iw)':-2" -t "$len" \
		-c:v libvpx-vp9 -b:v 0 -crf "$crf" -row-mt 1 -deadline good -cpu-used 2 -r 24 "$out"
}

encode_webp() {
	local in="$1" out="$2" q="$3" fps="$4"
	ffmpeg -loglevel error -y -ss "$trim" -i "$in" -an -vf "${pts}fps=$fps,scale='min(800,iw)':-2:flags=lanczos" -t "$len" \
		-c:v libwebp_anim -lossless 0 -q:v "$q" -compression_level 6 -loop 0 "$out"
}

encode_gif() {
	local in="$1" out="$2" width="$3" fps="$4" colors="$5"
	ffmpeg -loglevel error -y -ss "$trim" -i "$in" -an -t "$len" \
		-filter_complex "${pts}fps=$fps,scale='min($width,iw)':-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=$colors:stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=4:diff_mode=rectangle" \
		-loop 0 "$out"
}

shrink() {
	# $1 encoder fn, $2 in, $3 out, $4 budget bytes, then a list of quality steps
	local fn="$1" in="$2" out="$3" budget="$4"
	shift 4
	for q in "$@"; do
		"$fn" "$in" "$out" $q
		if [[ "$(size_of "$out")" -le "$budget" ]]; then return 0; fi
	done
	echo "warn: $out is over budget ($(mb "$(size_of "$out")") MB)" >&2
}

printf '%-14s %8s %8s %8s %8s %8s %6s %5s\n' slug mp4 webm webp gif poster secs speed
for slug in "${slugs[@]}"; do
	in="$raw/$slug.webm"
	[[ -f "$in" ]] || { echo "skip $slug: no recording" >&2; continue; }
	trim="${TRIM[$slug]:-1.0}"
	dur="$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$in")"
	max=15
	[[ "$slug" == hero ]] && max=35
	avail="$(awk -v d="$dur" -v t="$trim" 'BEGIN{printf "%.2f", d-t-0.3}')"
	speed="$(awk -v a="$avail" -v m="$max" 'BEGIN{s=a/m; if (s<1) s=1; printf "%.3f", s}')"
	len="$(awk -v a="$avail" -v s="$speed" 'BEGIN{printf "%.2f", a/s}')"
	pts="setpts=PTS/$speed,"
	mp4_budget=$((2500 * 1024))
	[[ "$slug" == hero ]] && mp4_budget=$((6 * 1024 * 1024))

	shrink encode_mp4 "$in" "$dest/$slug.mp4" "$mp4_budget" 20 23 26 29 32
	shrink encode_webp "$in" "$dest/$slug.webp" $((2 * 1024 * 1024)) "72 12" "60 12" "45 12" "35 10" "28 8" "22 6"
	poster_at="$(awk -v p="${POSTER[$slug]:-3}" -v t="$trim" 'BEGIN{printf "%.2f", p+t}')"
	ffmpeg -loglevel error -y -ss "$poster_at" -i "$in" -frames:v 1 -vf "scale='min(1280,iw)':-2" -c:v libwebp -q:v 82 "$dest/$slug.poster.webp"
	gif="-"
	if [[ -n "${GIF:-}" && " $GIF_SLUGS " == *" $slug "* ]]; then
		shrink encode_gif "$in" "$dest/$slug.gif" $((4 * 1024 * 1024)) "800 12 160" "800 10 128" "720 8 96" "640 8 64" "560 6 48"
		gif="$(mb "$(size_of "$dest/$slug.gif")")"
	fi
	printf '%-14s %8s %8s %8s %8s %8s %6s %5s\n' "$slug" "$(mb "$(size_of "$dest/$slug.mp4")")" - \
		"$(mb "$(size_of "$dest/$slug.webp")")" "$gif" "$(mb "$(size_of "$dest/$slug.poster.webp")")" "$len" "$speed"
done
