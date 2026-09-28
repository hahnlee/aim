#!/usr/bin/env python3
"""Small HTTPS origin used by the unmodified Chromium Android acceptance gate."""

from __future__ import annotations

import argparse
import http.server
import pathlib
import ssl
import urllib.parse


class AcceptanceHandler(http.server.SimpleHTTPRequestHandler):
    report_path: pathlib.Path

    def do_GET(self) -> None:
        parsed = urllib.parse.urlsplit(self.path)
        if parsed.path == "/report":
            values = urllib.parse.parse_qs(parsed.query, keep_blank_values=True)
            line = " ".join(
                f"{key}={','.join(values[key])}" for key in sorted(values)
            )
            with self.report_path.open("a", encoding="utf-8") as report:
                report.write(line + "\n")
            payload = b"ok\n"
            self.send_response(200)
            self.send_header("Content-Type", "text/plain; charset=utf-8")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
            return
        super().do_GET()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--directory", required=True)
    parser.add_argument("--report", required=True)
    parser.add_argument("--cert", required=True)
    parser.add_argument("--key", required=True)
    parser.add_argument("--port", required=True, type=int)
    args = parser.parse_args()

    directory = pathlib.Path(args.directory).resolve()
    report = pathlib.Path(args.report).resolve()
    report.parent.mkdir(parents=True, exist_ok=True)
    report.touch()
    AcceptanceHandler.report_path = report

    def handler(*handler_args, **handler_kwargs):
        return AcceptanceHandler(
            *handler_args, directory=str(directory), **handler_kwargs
        )

    server = http.server.ThreadingHTTPServer(("127.0.0.1", args.port), handler)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(args.cert, args.key)
    server.socket = context.wrap_socket(server.socket, server_side=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
