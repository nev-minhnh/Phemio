"""Turns ISLET_DUMP frames (<class>_<w>x<h>.bgra, premultiplied BGRA) into PNGs."""
import glob, os, sys
from PIL import Image

src, prefix = sys.argv[1], sys.argv[2]
for f in glob.glob(os.path.join(src, "*.bgra")):
    name = os.path.basename(f)[:-5]
    cls, size = name.rsplit("_", 1)
    w, h = map(int, size.split("x"))
    raw = open(f, "rb").read()
    if len(raw) != w * h * 4:
        continue
    im = Image.frombuffer("RGBA", (w, h), raw, "raw", "BGRA", 0, 1)
    bg = Image.new("RGBA", (w, h), (128, 132, 140, 255))
    r, g, b, a = im.split()
    bg.paste(Image.merge("RGB", (r, g, b)), (0, 0), a)
    bg.convert("RGB").save(f"{prefix}-{cls}.png")
