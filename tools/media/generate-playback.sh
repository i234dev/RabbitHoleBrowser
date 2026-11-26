#!/usr/bin/env bash
# Generate a self-contained MSE corpus outside the repository.
set -eu
out=${1:?supply an output directory}
mkdir -p "$out"
for size in 1280x720 1920x1080; do
    for codec in vp9 av1 h264; do
        case $codec in
            vp9)  encoder=(-c:v libvpx-vp9 -deadline realtime -cpu-used 6 -lag-in-frames 0); mime='video/webm; codecs="vp9"'; ext=webm ;;
            av1)  encoder=(-c:v libaom-av1 -usage realtime -cpu-used 8 -lag-in-frames 0); mime='video/webm; codecs="av01.0.08M.08"'; ext=webm ;;
            h264) encoder=(-c:v libx264 -preset veryfast -bf 0 -movflags +empty_moov+default_base_moof+frag_keyframe); mime='video/mp4; codecs="avc1.640028"'; ext=mp4 ;;
        esac
        name=$codec-$size
        "${FFMPEG:-ffmpeg}" -hide_banner -loglevel error -y -f lavfi -i "testsrc2=size=$size:rate=30" \
            -frames:v 90 -an -pix_fmt yuv420p -threads 3 -g 30 "${encoder[@]}" "$out/$name.$ext"
        cat > "$out/$name.html" <<HTML
<!doctype html><meta charset="utf-8"><style>*{margin:0}video{width:640px;height:360px}</style>
<video muted></video><script>
const video = document.querySelector("video");
const source = new MediaSource();
source.addEventListener("sourceopen", async () => {
    try {
        const buffer = source.addSourceBuffer('$mime');
        const response = await fetch('$name.$ext');
        const data = await response.arrayBuffer();
        buffer.addEventListener("updateend", () => {
            source.endOfStream();
            video.play().catch(e => console.error(String(e)));
        }, {once:true});
        buffer.appendBuffer(data);
    } catch (e) { console.error(String(e)); }
});
video.src = URL.createObjectURL(source);
</script>
HTML
    done
done
