#!/usr/bin/env python3
"""mock_api.py -- local OpenAI-compatible mock API for the `dst` TUI.

Pure Python standard library (http.server + threading).  Listens on
127.0.0.1 by default and serves just enough of the OpenAI wire protocol for dst:

  GET  /v1/models              model list, with `context_window` (and the
                               `effort.supported_levels` shape dst parses)
  POST /v1/chat/completions    streaming SSE *and* non-streaming, driven by a
                               JSON script of per-request responses:
                                 reasoning_content deltas (dst's thinking field)
                                 content deltas
                                 tool_calls deltas (id/name/arguments split
                                 across chunks, as the real API streams them)
                                 finish_reason, usage (incl. cache hit/miss)
  GET  /health, GET /          introspection

Requests are appended to a JSONL log (`--log`), one `request` record followed by
one `response` record per call.

Script format
-------------
Top level is either a list of response entries, or an object:
    {
      "models": [ ... ],            # optional, served by GET /v1/models
      "auto_tool": true,           # optional, same as --auto-tool
      "responses": [ entry, ... ]  # one entry per POST, consumed in order
    }

An entry is a convenience shorthand:
    {
      "initial_delay_ms": 200,        # sleep before the first chunk
      "reasoning": "text",            # or list of chunks; streamed as
                                      # delta.reasoning_content
      "reasoning_chunk_chars": 12,    # split a string into chunks this big
      "reasoning_delay_ms": 120,      # pause after each reasoning chunk
      "content": "text",              # or list of chunks -> delta.content
      "content_chunk_chars": 10,
      "content_delay_ms": 80,
      "tool_calls": [                 # streamed as delta.tool_calls
        {"id": "call_1", "name": "list_dir", "arguments": {"path": "."},
         "tool_chunk_chars": 8, "tool_delay_ms": 50}
      ],
      "finish_reason": "stop",        # default: tool_calls if tools, else stop
      "usage": {"prompt_tokens": 120, "completion_tokens": 42,
                "prompt_cache_hit_tokens": 96,
                "prompt_cache_miss_tokens": 24, "total_tokens": 162}
    }

An entry may fail the HTTP request instead of streaming (dst's provider-error
path, e.g. bad key or rate limit):
    {"http_error": {"code": 401, "message": "Authentication Fails",
                    "type": "authentication_error"}}

For exact control, an entry may instead carry explicit deltas:
    {"events": [
        {"type": "delay", "ms": 100},
        {"type": "reasoning", "text": "think", "delay_ms": 80},
        {"type": "content", "text": "Hi", "delay_ms": 40},
        {"type": "tool_call", "index": 0, "id": "call_1", "name": "list_dir"},
        {"type": "tool_call", "index": 0, "arguments": "{\"path\":\".\"}"},
        {"type": "finish", "reason": "tool_calls"},
        {"type": "usage", "usage": {...}},
        {"type": "raw", "chunk": {"choices": [...]}}
    ]}

When the script is exhausted the server falls back to a canned text reply.  With
`--auto-tool` (or `"auto_tool": true` in the script) an exhausted script first
emits a harmless `list_dir` tool call; when dst sends the tool result back the
server replies with a final text.  That is the "tool call then final reply" mode.

Examples
--------
    python3 mock_api.py --port 8765 --script sample_script.json \
        --log .work/requests.jsonl
    python3 mock_api.py --port 0 --auto-tool          # ephemeral port, print URL
"""

from __future__ import annotations

import argparse
import json
import os
import signal
import sys
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

DONE = "[DONE]"

DEFAULT_MODELS = [
    {
        "id": "deepseek-flash",
        "object": "model",
        "created": 1700000000,
        "owned_by": "deepseek",
        "name": "DeepSeek Flash",
        "context_window": 131072,
        "effort": {"supported_levels": ["auto", "low", "high", "max"]},
    },
    {
        "id": "deepseek-v4",
        "object": "model",
        "created": 1700000000,
        "owned_by": "deepseek",
        "name": "DeepSeek V4",
        "context_window": 1048576,
        "effort": {"supported_levels": ["auto", "high", "max"]},
    },
]


# --------------------------------------------------------------------------- #
# helpers
# --------------------------------------------------------------------------- #

def log(msg: str) -> None:
    sys.stdout.write(msg + "\n")
    sys.stdout.flush()


def dumps(obj) -> str:
    return json.dumps(obj, ensure_ascii=False, separators=(",", ":"))


def split_text(text: str, size: int):
    size = max(1, int(size or 1))
    return [text[i:i + size] for i in range(0, len(text), size)] or [""]


def as_chunks(value, chunk_chars):
    if value is None:
        return []
    if isinstance(value, list):
        return [str(v) for v in value if str(v) != ""]
    return split_text(str(value), chunk_chars)


def approx_tokens(text: str) -> int:
    if not text:
        return 0
    return max(1, int(len(text) / 4) + (1 if len(text) % 4 else 0))


def make_usage(messages, reasoning_text, content_text, n_tool_calls):
    prompt_chars = 0
    for m in messages or []:
        if isinstance(m, dict):
            prompt_chars += len(str(m.get("content") or ""))
            prompt_chars += 24  # tool_call / role overhead per message
    prompt = max(1, prompt_chars // 4)
    completion = max(1, approx_tokens(reasoning_text) + approx_tokens(content_text)
                     + 8 * n_tool_calls)
    hit = max(0, prompt - 48)
    miss = prompt - hit
    return {
        "prompt_tokens": prompt,
        "completion_tokens": completion,
        "prompt_cache_hit_tokens": hit,
        "prompt_cache_miss_tokens": miss,
        "total_tokens": prompt + completion,
    }


def normalize_tool_calls(entry):
    tcs = entry.get("tool_calls")
    if tcs is None and "tool_call" in entry:
        tcs = [entry["tool_call"]]
    out = []
    for i, tc in enumerate(tcs or []):
        if not isinstance(tc, dict):
            continue
        func = tc.get("function") if isinstance(tc.get("function"), dict) else {}
        name = tc.get("name") or func.get("name") or "list_dir"
        args = tc.get("arguments", func.get("arguments", "{}"))
        if args is None:
            args = "{}"
        if not isinstance(args, str):
            args = dumps(args)
        out.append({
            "index": i,
            "id": tc.get("id") or f"call_{uuid.uuid4().hex[:10]}",
            "name": str(name),
            "arguments": args,
            "tool_chunk_chars": int(tc.get("tool_chunk_chars",
                                           entry.get("tool_chunk_chars", 8))),
            "tool_delay_ms": int(tc.get("tool_delay_ms",
                                        entry.get("tool_delay_ms", 50))),
        })
    return out


def build_events(entry):
    """Turn a script entry into an ordered list of low-level stream events."""
    events = []
    initial = int(entry.get("initial_delay_ms",
                            entry.get("delay_before_ms", entry.get("delay_ms", 0))) or 0)
    if initial > 0:
        events.append({"type": "delay", "ms": initial})

    explicit = entry.get("events") or entry.get("deltas")
    if explicit is not None:
        if not isinstance(explicit, list):
            raise ValueError("entry 'events' must be a list")
        for raw in explicit:
            if not isinstance(raw, dict) or "type" not in raw:
                raise ValueError(f"bad event {raw!r}")
            ev = dict(raw)
            if ev["type"] == "tool_call" and "arguments" in ev and not isinstance(
                    ev["arguments"], str):
                ev["arguments"] = dumps(ev["arguments"])
            if ev["type"] not in ("delay", "reasoning", "content", "tool_call",
                                  "finish", "usage", "raw"):
                raise ValueError(f"unknown event type {ev['type']!r}")
            events.append(ev)
        return events

    reasoning = entry.get("reasoning", entry.get("reasoning_content",
                                                 entry.get("thinking")))
    r_chunk = int(entry.get("reasoning_chunk_chars", entry.get("chunk_chars", 12)))
    r_delay = int(entry.get("reasoning_delay_ms", entry.get("delta_delay_ms", 90)))
    for chunk in as_chunks(reasoning, r_chunk):
        events.append({"type": "reasoning", "text": chunk, "delay_ms": r_delay})

    content = entry.get("content")
    c_chunk = int(entry.get("content_chunk_chars", entry.get("chunk_chars", 10)))
    c_delay = int(entry.get("content_delay_ms", entry.get("delta_delay_ms", 70)))
    for chunk in as_chunks(content, c_chunk):
        events.append({"type": "content", "text": chunk, "delay_ms": c_delay})

    for tc in normalize_tool_calls(entry):
        piece_size = max(1, tc["tool_chunk_chars"])
        pieces = split_text(tc["arguments"], piece_size)
        for i, piece in enumerate(pieces):
            ev = {
                "type": "tool_call",
                "index": tc["index"],
                "delay_ms": tc["tool_delay_ms"],
            }
            if i == 0:
                ev["id"] = tc["id"]
                ev["name"] = tc["name"]
            ev["arguments"] = piece
            events.append(ev)
    return events


def entry_texts(entry):
    """Best-effort (reasoning, content, tool_calls) for non-streaming/logging."""
    reasoning = entry.get("reasoning", entry.get("reasoning_content",
                                                 entry.get("thinking")))
    if isinstance(reasoning, list):
        reasoning = "".join(str(x) for x in reasoning)
    content = entry.get("content")
    if isinstance(content, list):
        content = "".join(str(x) for x in content)
    return reasoning or "", content or ""


# --------------------------------------------------------------------------- #
# server state
# --------------------------------------------------------------------------- #

class MockState:
    def __init__(self, script_path=None, log_path=None, auto_tool=False):
        self.script_path = script_path
        self.log_path = log_path
        self.models = list(DEFAULT_MODELS)
        self.responses = []
        self.auto_tool = bool(auto_tool)
        self.script_meta = {}
        if script_path:
            with open(script_path, "r", encoding="utf-8") as fh:
                doc = json.load(fh)
            if isinstance(doc, list):
                self.responses = doc
            elif isinstance(doc, dict):
                self.responses = doc.get("responses") or doc.get("turns") or []
                if "models" in doc and isinstance(doc["models"], list):
                    self.models = doc["models"]
                self.auto_tool = bool(doc.get("auto_tool", self.auto_tool))
                self.script_meta = {k: v for k, v in doc.items()
                                    if k not in ("responses", "turns", "models")}
            else:
                raise SystemExit(f"mock_api: bad script {script_path!r}")
            for i, entry in enumerate(self.responses):
                if not isinstance(entry, dict):
                    raise SystemExit(f"mock_api: script entry {i} is not an object")
                build_events(entry)  # validate eagerly
        self.lock = threading.Lock()
        self.count = 0
        self.log_lock = threading.Lock()

    def next_entry(self):
        with self.lock:
            idx = self.count
            self.count += 1
        if idx < len(self.responses):
            return idx, self.responses[idx], "script"
        return None, None, "fallback"

    def remaining(self):
        with self.lock:
            return max(0, len(self.responses) - self.count)

    def append_log(self, record):
        if not self.log_path:
            return
        line = dumps(record) + "\n"
        with self.log_lock:
            os.makedirs(os.path.dirname(os.path.abspath(self.log_path)), exist_ok=True)
            with open(self.log_path, "a", encoding="utf-8") as fh:
                fh.write(line)


def fallback_entry(state, body):
    """Canned reply when the script runs out (or --auto-tool kicks in)."""
    messages = body.get("messages") or []
    roles = [m.get("role") for m in messages if isinstance(m, dict)]
    has_tool_result = bool(roles) and roles[-1] == "tool"
    tool_name = ""
    for m in messages:
        if isinstance(m, dict) and m.get("role") == "assistant" and m.get("tool_calls"):
            tool_name = m["tool_calls"][0].get("function", {}).get("name", "")
    if state.auto_tool and not has_tool_result:
        return {
            "content": "Let me check the working directory first.",
            "tool_calls": [{
                "id": f"call_{uuid.uuid4().hex[:8]}",
                "name": "list_dir",
                "arguments": {"path": "."},
            }],
            "finish_reason": "tool_calls",
            "auto": True,
        }
    if has_tool_result:
        what = f"`{tool_name}`" if tool_name else "the tool"
        return {
            "content": f"Done: {what} finished and its result came back. "
                       "This is the follow-up reply generated by mock_api.py.",
            "finish_reason": "stop",
            "auto": True,
        }
    return {
        "content": "Mock reply (script exhausted): dst is talking to the local "
                   "mock_api.py server, not the network.",
        "finish_reason": "stop",
        "auto": True,
    }


# --------------------------------------------------------------------------- #
# HTTP handler
# --------------------------------------------------------------------------- #

class MockHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.0"   # close-delimited bodies: simple SSE streaming
    server_version = "dst-mock-api/0.1"

    # silence the default per-request stderr logging
    def log_message(self, fmt, *args):  # noqa: A003
        pass

    @property
    def state(self) -> MockState:
        return self.server.state

    # -- helpers ----------------------------------------------------------- #
    def send_json(self, code: int, obj):
        payload = dumps(obj).encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(payload)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        try:
            self.wfile.write(payload)
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass

    def send_error_json(self, code: int, message: str, err_type="invalid_request_error"):
        self.send_json(code, {"error": {"message": message, "type": err_type,
                                        "code": code}})

    def read_body(self):
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length) if length else b""
        if not raw:
            return None, "empty request body"
        try:
            return json.loads(raw.decode("utf-8")), None
        except (UnicodeDecodeError, json.JSONDecodeError) as exc:
            return None, f"invalid JSON body: {exc}"

    # -- GET --------------------------------------------------------------- #
    def do_GET(self):
        path = self.path.split("?")[0].rstrip("/") or "/"
        state = self.state
        if path in ("/v1/models", "/models"):
            self.send_json(200, {"object": "list", "data": state.models})
            return
        if path.startswith("/v1/models/") or path.startswith("/models/"):
            model_id = path.rsplit("/", 1)[-1]
            for m in state.models:
                if m.get("id") == model_id:
                    self.send_json(200, m)
                    return
            self.send_error_json(404, f"model {model_id!r} not found", "not_found")
            return
        if path in ("/health", "/v1/health"):
            self.send_json(200, {
                "ok": True,
                "requests_served": state.count,
                "script": state.script_path,
                "responses_total": len(state.responses),
                "responses_remaining": state.remaining(),
                "auto_tool": state.auto_tool,
            })
            return
        if path == "/":
            self.send_json(200, {
                "service": "mock_api.py for dst",
                "endpoints": ["GET /v1/models", "POST /v1/chat/completions",
                              "GET /health"],
                "script": state.script_path,
                "log": state.log_path,
            })
            return
        self.send_error_json(404, f"no route for GET {path}", "not_found")

    # -- POST -------------------------------------------------------------- #
    def do_POST(self):
        path = self.path.split("?")[0].rstrip("/")
        if path not in ("/v1/chat/completions", "/chat/completions"):
            self.send_error_json(404, f"no route for POST {path}", "not_found")
            return
        body, err = self.read_body()
        if err:
            self.send_error_json(400, err)
            return
        if not isinstance(body, dict):
            self.send_error_json(400, "request body must be a JSON object")
            return

        state = self.state
        messages = body.get("messages") or []
        tools = body.get("tools") or []
        stream = bool(body.get("stream"))
        model = body.get("model") or (state.models[0].get("id") if state.models else "mock")
        idx, entry, source = state.next_entry()
        use_auto = False
        if entry is None:
            entry = fallback_entry(state, body)
            use_auto = True
        auto_flag = bool(entry.pop("auto", False)) if isinstance(entry, dict) else False

        req_no = idx + 1 if idx is not None else state.count
        log(f"[mock_api] req {req_no}: stream={stream} model={model!r} "
            f"messages={len(messages)} tools={len(tools)} "
            f"-> {source}{'' if idx is None else f' entry={idx + 1}'}"
            f"{' (tool-mode)' if (use_auto and auto_flag) else ''}")

        state.append_log({
            "type": "request",
            "request": req_no,
            "time": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
            "method": self.command,
            "path": self.path,
            "authorization": self.headers.get("Authorization"),
            "stream": stream,
            "script_source": source,
            "script_entry": None if idx is None else idx + 1,
            "body": body,
        })

        # A script entry can fail the request instead of streaming, e.g.
        # {"http_error": {"code": 401, "message": "invalid api key"}} to
        # exercise dst's provider-error path. The request is already logged.
        http_error = entry.get("http_error") if isinstance(entry, dict) else None
        if isinstance(http_error, dict):
            code = int(http_error.get("code", 500))
            message = str(http_error.get("message", "mock error"))
            err_type = str(http_error.get("type", "invalid_request_error"))
            log(f"[mock_api] req {req_no}: http_error {code} {message!r}")
            state.append_log({
                "type": "response",
                "request": req_no,
                "http_error": code,
                "message": message,
            })
            self.send_error_json(code, message, err_type)
            return

        try:
            if stream:
                summary = self.stream_completion(entry, body, model, req_no)
            else:
                summary = self.json_completion(entry, body, model, req_no)
        except (BrokenPipeError, ConnectionResetError) as exc:
            log(f"[mock_api] req {req_no}: client disconnected ({exc.__class__.__name__})")
            state.append_log({
                "type": "response", "request": req_no, "aborted": True,
                "error": exc.__class__.__name__,
            })
            return
        except Exception as exc:  # noqa: BLE001
            log(f"[mock_api] req {req_no}: script error {exc!r}")
            state.append_log({"type": "response", "request": req_no,
                              "error": repr(exc)})
            try:
                self.send_error_json(500, f"mock script error: {exc}", "server_error")
            except (BrokenPipeError, ConnectionResetError):
                pass
            return

        log(f"[mock_api] req {req_no}: done reasoning={summary['reasoning_chars']}ch "
            f"content={summary['content_chars']}ch tools={summary['tool_calls']} "
            f"finish={summary['finish_reason']} chunks={summary['chunks']}")
        state.append_log({
            "type": "response",
            "request": req_no,
            "script_source": source,
            "summary": summary,
        })

    # -- streaming --------------------------------------------------------- #
    def chunk_obj(self, cid, created, model, delta, finish=None, usage=None):
        obj = {
            "id": cid,
            "object": "chat.completion.chunk",
            "created": created,
            "model": model,
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
        }
        if usage is not None:
            obj["usage"] = usage
        return obj

    def stream_completion(self, entry, body, model, req_no):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream; charset=utf-8")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.send_header("X-Accel-Buffering", "no")
        self.end_headers()

        cid = f"chatcmpl-mock-{uuid.uuid4().hex[:12]}"
        created = int(time.time())
        events = build_events(entry)
        explicit_finish = entry.get("finish_reason")
        usage = entry.get("usage")
        acc_reason, acc_content = [], []
        tool_names = []
        chunks = 0

        def send(obj):
            nonlocal chunks
            if obj == DONE:
                self.wfile.write(b"data: [DONE]\n\n")
            else:
                self.wfile.write(("data: " + dumps(obj) + "\n\n").encode("utf-8"))
            self.wfile.flush()
            chunks += 1

        # OpenAI streams an initial role chunk before deltas.
        send(self.chunk_obj(cid, created, model,
                            {"role": "assistant", "content": ""}, None, None))

        finish_reason = explicit_finish
        saw_tool = False
        for ev in events:
            kind = ev["type"]
            delay = int(ev.get("delay_ms", ev.get("ms", 0)) or 0)
            if kind == "delay":
                time.sleep(delay / 1000.0)
                continue
            if kind == "reasoning":
                text = str(ev.get("text", ""))
                acc_reason.append(text)
                send(self.chunk_obj(cid, created, model,
                                    {"reasoning_content": text}, None, None))
                if delay:
                    time.sleep(delay / 1000.0)
            elif kind == "content":
                text = str(ev.get("text", ""))
                acc_content.append(text)
                send(self.chunk_obj(cid, created, model,
                                    {"content": text}, None, None))
                if delay:
                    time.sleep(delay / 1000.0)
            elif kind == "tool_call":
                saw_tool = True
                idx = int(ev.get("index", 0))
                func = {}
                if ev.get("name") is not None:
                    func["name"] = str(ev["name"])
                    if func["name"] not in tool_names:
                        tool_names.append(func["name"])
                if ev.get("arguments") is not None:
                    func["arguments"] = str(ev["arguments"])
                delta_tc = {"index": idx, "type": "function", "function": func}
                if ev.get("id") is not None:
                    delta_tc["id"] = str(ev["id"])
                send(self.chunk_obj(cid, created, model,
                                    {"tool_calls": [delta_tc]}, None, None))
                if delay:
                    time.sleep(delay / 1000.0)
            elif kind == "finish":
                finish_reason = str(ev.get("reason") or finish_reason or "stop")
            elif kind == "usage":
                usage = ev.get("usage")
            elif kind == "raw":
                send(ev.get("chunk") or {})
            if self._client_gone():
                raise BrokenPipeError("client went away")

        if usage is None:
            usage = make_usage(body.get("messages") or [],
                               "".join(acc_reason), "".join(acc_content),
                               len(tool_names))
        if finish_reason is None:
            finish_reason = "tool_calls" if saw_tool else "stop"
        # dst only reads `usage` from a chunk that also has choices[0], so the
        # usage rides on the finish chunk (plus a spec-shaped usage-only chunk).
        send(self.chunk_obj(cid, created, model, {}, finish_reason, usage))
        send({"id": cid, "object": "chat.completion.chunk", "created": created,
              "model": model, "choices": [], "usage": usage})
        send(DONE)
        self.close_connection = True
        return {
            "reasoning_chars": sum(len(x) for x in acc_reason),
            "content_chars": sum(len(x) for x in acc_content),
            "tool_calls": len(tool_names),
            "finish_reason": finish_reason,
            "usage": usage,
            "chunks": chunks,
        }

    def _client_gone(self):
        try:
            return self.wfile.closed or self.connection is None
        except Exception:
            return True

    # -- non-streaming ----------------------------------------------------- #
    def json_completion(self, entry, body, model, req_no):
        reasoning, content = entry_texts(entry)
        tool_calls = normalize_tool_calls(entry)
        message = {"role": "assistant"}
        if content:
            message["content"] = content
        else:
            message["content"] = None
        if reasoning:
            message["reasoning_content"] = reasoning
        if tool_calls:
            message["tool_calls"] = [
                {
                    "id": tc["id"],
                    "type": "function",
                    "function": {"name": tc["name"], "arguments": tc["arguments"]},
                }
                for tc in tool_calls
            ]
        finish = entry.get("finish_reason") or (
            "tool_calls" if tool_calls else "stop")
        usage = entry.get("usage") or make_usage(
            body.get("messages") or [], reasoning, content, len(tool_calls))
        cid = f"chatcmpl-mock-{uuid.uuid4().hex[:12]}"
        out = {
            "id": cid,
            "object": "chat.completion",
            "created": int(time.time()),
            "model": model,
            "choices": [{"index": 0, "message": message, "finish_reason": finish,
                         "logprobs": None}],
            "usage": usage,
        }
        self.send_json(200, out)
        return {
            "reasoning_chars": len(reasoning),
            "content_chars": len(content),
            "tool_calls": len(tool_calls),
            "finish_reason": finish,
            "usage": usage,
            "chunks": 1,
        }


class MockServer(ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True

    def __init__(self, addr, state):
        self.state = state
        super().__init__(addr, MockHandler)


# --------------------------------------------------------------------------- #
# main
# --------------------------------------------------------------------------- #

def main(argv=None):
    p = argparse.ArgumentParser(
        prog="mock_api.py",
        description="Local OpenAI-compatible mock API for the dst TUI.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "examples:\n"
            "  %(prog)s --port 8765 --script sample_script.json --log requests.jsonl\n"
            "  %(prog)s --port 0 --auto-tool      # print the ephemeral URL\n"
        ),
    )
    p.add_argument("--host", default="127.0.0.1", help="bind host (default 127.0.0.1)")
    p.add_argument("--port", type=int, default=8765,
                   help="bind port; 0 = ephemeral (default 8765)")
    p.add_argument("--script", default=None,
                   help="JSON script of per-request responses (optional)")
    p.add_argument("--log", default=None,
                   help="JSONL request/response log path "
                        "(default: <script>.requests.jsonl or mock_requests.jsonl)")
    p.add_argument("--auto-tool", action="store_true",
                   help="after the script runs out: trigger a list_dir tool call, "
                        "then a final text reply")
    p.add_argument("--quiet", action="store_true", help="no per-request stdout lines")
    args = p.parse_args(argv)

    if args.log is None:
        if args.script:
            base, _ = os.path.splitext(args.script)
            args.log = base + ".requests.jsonl"
        else:
            args.log = "mock_requests.jsonl"

    state = MockState(args.script, args.log, args.auto_tool)
    try:
        server = MockServer((args.host, args.port), state)
    except OSError as exc:
        sys.stderr.write(f"mock_api: cannot bind {args.host}:{args.port}: {exc}\n")
        return 2
    port = server.server_address[1]
    url = f"http://{args.host}:{port}/v1"

    def on_signal(signum, frame):
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, on_signal)
    signal.signal(signal.SIGINT, on_signal)

    log(f"mock_api: serving {url}")
    log(f"MOCK_API_URL={url}")
    log(f"MOCK_API_PORT={port}")
    log(f"mock_api: script={args.script or '(none)'} responses={len(state.responses)} "
        f"log={args.log} auto_tool={state.auto_tool}")
    if state.script_meta:
        log(f"mock_api: script meta={dumps(state.script_meta)}")
    try:
        server.serve_forever(poll_interval=0.2)
    except KeyboardInterrupt:
        log("mock_api: shutting down")
    finally:
        server.server_close()
        log(f"mock_api: {state.count} request(s) served")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
