from __future__ import annotations

import argparse
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


MODEL = "rampage-test:latest"
DIGEST = "a" * 64


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self) -> None:
        if self.path == "/api/tags":
            self._json(
                {
                    "models": [
                        {
                            "name": MODEL,
                            "model": MODEL,
                            "size": 1024,
                            "digest": DIGEST,
                            "details": {"format": "gguf"},
                        }
                    ]
                }
            )
            return
        if self.path == "/api/version":
            self._json({"version": "rampage-e2e"})
            return
        self.send_error(404)

    def do_POST(self) -> None:
        if self.path != "/api/chat":
            self.send_error(404)
            return
        length = int(self.headers.get("content-length", "0"))
        if length <= 0 or length > 1024 * 1024:
            self.send_error(400)
            return
        request = json.loads(self.rfile.read(length))
        if request.get("model") != MODEL or request.get("stream") is not True:
            self.send_error(400)
            return
        prompt = " ".join(
            str(message.get("content", "")) for message in request.get("messages", [])
        )
        answer = "STREAM_OK" if "STREAM_OK" in prompt else "RAMPAGE_OK"
        frames = [
            {
                "model": MODEL,
                "message": {"role": "assistant", "content": answer[:4]},
                "done": False,
            },
            {
                "model": MODEL,
                "message": {"role": "assistant", "content": answer[4:]},
                "done": True,
                "done_reason": "stop",
                "prompt_eval_count": 7,
                "eval_count": 2,
            },
        ]
        payload = b"".join(
            json.dumps(frame, separators=(",", ":")).encode("utf-8") + b"\n"
            for frame in frames
        )
        self.send_response(200)
        self.send_header("content-type", "application/x-ndjson")
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def _json(self, value: object) -> None:
        payload = json.dumps(value, separators=(",", ":")).encode("utf-8")
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, format: str, *args: object) -> None:
        return


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, required=True)
    args = parser.parse_args()
    ThreadingHTTPServer(("127.0.0.1", args.port), Handler).serve_forever()


if __name__ == "__main__":
    main()
