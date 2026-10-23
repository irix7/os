#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""HTTP server with byte-range GETs, for the guest `erofs` command.

Usage: range-server.py [--port PORT] [--directory DIR]

The guest fetches EROFS blocks with `Range: bytes=start-end`, so a plain
`python3 -m http.server` is not enough.  This is a local test helper; it is
not part of the shipped image.
"""

import argparse
import functools
import http.server
import os
import re


class RangeHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, fmt, *args):
        print("%s - %s" % (self.address_string(), fmt % args))

    def do_GET(self):
        path = self.translate_path(self.path)
        if not os.path.isfile(path):
            self.send_error(404)
            return
        size = os.path.getsize(path)
        start, end = 0, size - 1
        status = 200
        range_header = self.headers.get("Range")
        if range_header:
            match = re.fullmatch(r"bytes=(\d+)-(\d*)", range_header.strip())
            if not match:
                self.send_error(400)
                return
            start = int(match.group(1))
            end = int(match.group(2)) if match.group(2) else size - 1
            end = min(end, size - 1)
            if start > end or start >= size:
                self.send_response(416)
                self.send_header("Content-Range", f"bytes */{size}")
                self.send_header("Content-Length", "0")
                self.end_headers()
                return
            status = 206

        length = end - start + 1
        self.send_response(status)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Content-Length", str(length))
        if status == 206:
            self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
        self.end_headers()
        with open(path, "rb") as image:
            image.seek(start)
            self.wfile.write(image.read(length))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=18082)
    parser.add_argument("--directory", default=".")
    args = parser.parse_args()
    handler = functools.partial(RangeHandler, directory=args.directory)
    server = http.server.ThreadingHTTPServer(("0.0.0.0", args.port), handler)
    print(f"serving {args.directory} on port {args.port}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
