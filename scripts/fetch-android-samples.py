"""Download a few Leica sample DNGs (an M9 first) into the Android app's assets.

The samples come from raw.pixls.us, whose files are public domain (CC0), so
they can ship inside a public APK. They are fetched at build time and never
committed (android/app/src/main/assets/samples is git-ignored).

    python3 scripts/fetch-android-samples.py [count]

Never fails the build: if the site is unreachable it warns and the APK is
built without samples (the app then hides its Samples button).
"""

import html.parser
import os
import sys
import urllib.parse
import urllib.request

BASE = "https://raw.pixls.us/data/"
# raw.pixls.us has a single M9 file, so the rest come from other Leica bodies
# (the app's films adapt to each sensor). Folders that don't exist are skipped.
CANDIDATE_DIRS = [
    "Leica/M9/",
    "Leica/M10/",
    "Leica/M (Typ 240)/",
    "Leica/M8/",
    "Leica/Q (Typ 116)/",
    "Leica/M11/",
    "Leica/Q2/",
]
OUT = os.path.join(os.path.dirname(__file__), "..", "android", "app", "src", "main", "assets", "samples")
MAX_BYTES = 40 * 1024 * 1024


class Links(html.parser.HTMLParser):
    def __init__(self):
        super().__init__()
        self.hrefs = []

    def handle_starttag(self, tag, attrs):
        if tag == "a":
            self.hrefs += [v for k, v in attrs if k == "href" and v]


def get(url, limit=None):
    req = urllib.request.Request(url, headers={"User-Agent": "remjet-ci"})
    with urllib.request.urlopen(req, timeout=60) as r:
        return r.read(limit) if limit else r.read()


def warn(msg):
    print(f"::warning::{msg}")


def main():
    count = int(sys.argv[1]) if len(sys.argv) > 1 else 3
    os.makedirs(OUT, exist_ok=True)
    got, sources = 0, []
    for d in CANDIDATE_DIRS:
        if got >= count:
            break
        url = BASE + urllib.parse.quote(d)
        try:
            page = get(url).decode("utf-8", "replace")
        except Exception as e:  # noqa: BLE001 - any failure: try the next folder
            print(f"{url}: {e}")
            continue
        parser = Links()
        parser.feed(page)
        dngs = sorted({urllib.parse.urljoin(url, h) for h in parser.hrefs if h.lower().split("?")[0].endswith(".dng")})
        print(f"{url}: {len(dngs)} DNG links")
        if not dngs:
            print(page[:2000])
            continue
        camera = d.strip("/").replace("/", " ")
        for link in dngs:
            if got >= count:
                break
            name = os.path.basename(urllib.parse.unquote(urllib.parse.urlparse(link).path))
            try:
                data = get(link, MAX_BYTES + 1)
            except Exception as e:  # noqa: BLE001
                print(f"{link}: {e}")
                continue
            if len(data) > MAX_BYTES or not data[:4] in (b"II*\x00", b"MM\x00*"):
                print(f"{link}: skipped (not a DNG or too big)")
                continue
            name = f"{camera} {name}"  # shown in the app's Samples list
            with open(os.path.join(OUT, name), "wb") as f:
                f.write(data)
            got += 1
            sources.append(link)
            print(f"{name}: {len(data) // 1024} KB")
            break  # one per camera, for variety
    if not got:
        warn("No sample DNGs downloaded; the APK is built without samples.")
        return
    with open(os.path.join(OUT, "SOURCE.txt"), "w") as f:
        f.write("Public domain (CC0) sample DNGs from https://raw.pixls.us\n" + "\n".join(sources) + "\n")


if __name__ == "__main__":
    main()
