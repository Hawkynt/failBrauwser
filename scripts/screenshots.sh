#!/usr/bin/env bash
# Renders the README screenshots in a headless compositor with the current desktop theme.
#   scripts/screenshots.sh            → docs/screenshots/*.png
#   scripts/screenshots.sh NAME…      → only those (main, vcs-overlays, link-overlays, …)
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
ONLY=" $* "
WORK="$ROOT/target/screenshots"
DEMO="$WORK/home"
OUT="$ROOT/docs/screenshots"
mkdir -p "$OUT"
if [ -d "$WORK" ]; then find "$WORK" -mindepth 1 -delete; fi
mkdir -p "$DEMO"

python3 - "$DEMO" <<'PY'
import io, os, random, sys, tarfile, time, zipfile
home = sys.argv[1]
random.seed(7)
def f(path, size):
    p = os.path.join(home, path)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, "wb") as fh:
        fh.write(random.randbytes(size))
    t = time.time() - random.randint(0, 90) * 86400
    os.utime(p, (t, t))
for n in ["Budget 2026.ods", "Letter to landlord.odt", "Thesis draft.pdf", "notes.txt"]:
    f("Documents/" + n, random.randint(8_000, 900_000))
for i in range(1, 9):
    f(f"Music/Night Drive/{i:02d} - Track {i}.flac", random.randint(900_000, 3_000_000))
# Real pictures, so thumbnails have something to show.
from PIL import Image, ImageDraw
os.makedirs(os.path.join(home, "Pictures/Vacation"), exist_ok=True)
for i in range(1, 25):
    w, h = 1200, 800
    top = (random.randint(40, 120), random.randint(120, 200), random.randint(180, 255))
    bottom = (random.randint(180, 255), random.randint(120, 200), random.randint(40, 120))
    im = Image.new("RGB", (w, h))
    d = ImageDraw.Draw(im)
    for y in range(h):
        t = y / h
        d.line([(0, y), (w, y)], fill=tuple(int(a + (b - a) * t) for a, b in zip(top, bottom)))
    d.ellipse([random.randint(100, 800), random.randint(80, 300), random.randint(900, 1150), random.randint(400, 700)], fill=(255, 230, 120))
    d.polygon([(0, h), (random.randint(200, 600), random.randint(350, 550)), (w, h)], fill=(40, 90, 60))
    im.save(os.path.join(home, f"Pictures/Vacation/IMG_{2400 + i}.jpg"), quality=85)
for n in ["cat.png", "sunset.jpg", "wallpaper.png"]:
    f("Pictures/" + n, random.randint(200_000, 4_000_000))
for n in ["index.html", "style.css", "app.js", "README.md"]:
    f("Projects/website/" + n, random.randint(1_000, 40_000))
f("Videos/birthday.mp4", 6_000_000)
f("todo.md", 1200)
f("Desktop/notes.txt", 2400)
# Real archives to browse into.
dl = os.path.join(home, "Downloads"); os.makedirs(dl, exist_ok=True)
with zipfile.ZipFile(os.path.join(dl, "photos-2024.zip"), "w", zipfile.ZIP_DEFLATED) as z:
    for season in ["spring", "summer", "winter"]:
        for i in range(1, 7):
            z.writestr(f"2024/{season}/DSC_{i:04d}.jpg", random.randbytes(random.randint(50_000, 400_000)))
    z.writestr("2024/index.txt", "photo index\n")
inner = io.BytesIO()
with tarfile.open(fileobj=inner, mode="w:gz") as t:
    for n in ["config.toml", "data.db", "logs/app.log"]:
        data = random.randbytes(random.randint(2_000, 90_000))
        info = tarfile.TarInfo("server/" + n); info.size = len(data); info.mtime = time.time()
        t.addfile(info, io.BytesIO(data))
with zipfile.ZipFile(os.path.join(dl, "backup.zip"), "w", zipfile.ZIP_DEFLATED) as z:
    z.writestr("server-backup.tar.gz", inner.getvalue())
    z.writestr("README.txt", "Backups of the home server\n")
PY

# Links: a hard link, symbolic links to a file and a folder, a link whose target is gone.
L="$DEMO/Links"
mkdir -p "$L"
printf 'Quarterly report\n' > "$L/report.txt"
ln "$L/report.txt" "$L/report (hardlink).txt"
ln -s report.txt "$L/latest report"
ln -s ../Music "$L/Music"
ln -s draft-2023.txt "$L/old draft"
printf 'nothing special\n' > "$L/notes.txt"

# A CVS working copy its own client made, in every state: unchanged, changed, added, a conflict
# with a commit from elsewhere, unknown, ignored (and a symbolic link: both overlays at once).
if command -v cvs >/dev/null; then
    CV="$WORK/cvsroot"
    cvs -Q -d "$CV" init
    I="$WORK/import"
    mkdir -p "$I/docs"
    printf 'int score = 0;\nint main(void) { return score; }\n' > "$I/main.c"
    printf 'int twice(int x) { return 2 * x; }\n' > "$I/util.c"
    printf 'A small game.\n' > "$I/README"
    printf 'How to play.\n' > "$I/docs/manual.txt"
    printf '*.o\n' > "$I/.cvsignore"
    (cd "$I" && cvs -Q -d "$CV" import -m "first" game vendor start)
    (cd "$DEMO/Projects" && cvs -Q -d "$CV" checkout game)
    (cd "$WORK" && cvs -Q -d "$CV" checkout -d game2 game && cd game2 && sed -i 's/score = 0/score = 10/' main.c && cvs -Q commit -m second)
    G="$DEMO/Projects/game"
    sleep 1.1 # CVS compares times in whole seconds
    sed -i 's/score = 0/score = 99/' "$G/main.c"
    (cd "$G" && cvs -Q update >/dev/null 2>&1) || true
    sleep 1.1
    printf 'int half(int x) { return x / 2; }\n' >> "$G/util.c"
    printf 'void beep(void) {}\n' > "$G/sound.c"
    (cd "$G" && cvs -Q add sound.c 2>/dev/null)
    printf 'try this\n' > "$G/ideas.txt"
    printf 'object\n' > "$G/main.o"
    ln -s main.c "$G/latest.c"
fi

shoot() { # name, script lines…
    local name=$1; shift
    if [ "$ONLY" != "  " ] && [[ "$ONLY" != *" $name "* ]]; then return 0; fi
    local t="$WORK/run-$name"; mkdir -p "$t"
    printf '%s\n' "$@" > "$t/steps.fbt"
    FB_THEME_HOME="$HOME" HOME="$DEMO" "$ROOT/tests/ui/harness.sh" "$ROOT/target/release/failbrauwser" "$t/steps.fbt" "$t" | grep -q "SELFTEST OK" \
        || { echo "screenshot $name failed" >&2; exit 1; }
}

(cd "$ROOT" && cargo build --release)
shoot main "action win.tree-root home" "open $DEMO" "select Pictures" "sleep 1500" "screenshot $OUT/main.png"
shoot archive "open $DEMO/Downloads/photos-2024.zip/2024" "select summer" "sleep 1000" "screenshot $OUT/archive.png"
shoot nested "open $DEMO/Downloads/backup.zip/server-backup.tar.gz/server" "select config.toml" "sleep 800" "screenshot $OUT/nested-archive.png"
shoot drives "action win.drives" "wait-drive File System" "sleep 800" "screenshot $OUT/drives.png"
shoot icons "action win.view-mode icons" "action win.zoom-in" "open $DEMO/Pictures/Vacation" "wait-thumbnails 12" "sleep 800" "screenshot $OUT/icons.png"
shoot search "open $DEMO" "find img_241" "find-options on off" "wait-row IMG_2411.jpg" "sleep 600" "screenshot $OUT/search.png"
shoot properties "open $DEMO" "select Music" "action win.properties" "sleep 1200" "screenshot $OUT/properties.png"
shoot conflict "open $DEMO/Documents" "select notes.txt" "action win.copy" "open $DEMO/Desktop" "action win.paste" "sleep 800" "screenshot $OUT/copy-conflict.png"
shoot trash "open $DEMO/Documents" "select Letter to landlord.odt | notes.txt" "action win.trash" "wait-missing $DEMO/Documents/notes.txt" \
    "open trash:///" "wait-row notes.txt" "select notes.txt" "sleep 800" "screenshot $OUT/trash.png"
shoot drive-properties "action win.drives" "wait-drive File System" "drive-properties File System" "sleep 1000" "screenshot $OUT/drive-properties.png"
shoot vcs-overlays "action app.vcs-integration" "action win.view-mode icons" "action win.zoom-in" "open $DEMO/Projects/game" "wait-git-overlay main.c = Conflicted" \
    "wait-git-overlay util.c = Modified" "wait-git-overlay sound.c = Added" "wait-git-overlay ideas.txt = Unversioned" "wait-git-overlay README = Normal" \
    "select README" "sleep 800" "screenshot $OUT/vcs-overlays.png"
shoot link-overlays "action win.view-mode icons" "action win.zoom-in" "open $DEMO/Links" "expect-link-overlay report.txt = hard" \
    "expect-link-overlay latest report = symbolic" "expect-link-overlay old draft = broken" "select notes.txt" "sleep 800" "screenshot $OUT/link-overlays.png"
echo "screenshots written to $OUT"
