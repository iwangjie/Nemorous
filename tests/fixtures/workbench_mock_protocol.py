"""Local synthetic DAP and LSP peer used by the headless workbench tests."""
import json
from pathlib import Path
import sys

mode, proof = sys.argv[1:]
proof = Path(proof)


def read():
    headers = {}
    while line := sys.stdin.buffer.readline():
        if line == b"\r\n":
            return json.loads(sys.stdin.buffer.read(int(headers[b"Content-Length"])))
        key, value = line.split(b":", 1)
        headers[key] = value.strip()
    return None


def send(payload):
    body = json.dumps(payload, ensure_ascii=False).encode()
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode() + body)
    sys.stdout.buffer.flush()


def event(name, body=None):
    send(dict(type="event", seq=99, event=name, body=body or {}))


while incoming := read():
    if mode == "dap" and incoming.get("type") == "request":
        command = incoming["command"]
        arguments = incoming.get("arguments", {})
        with proof.open("a") as log:
            log.write(f"{command} {json.dumps(arguments)}\n")
        if command == "launch":
            event("initialized")
        body = {
            "initialize": {"supportsConfigurationDoneRequest": True},
            "stackTrace": {"stackFrames": [{"id": 3, "name": "fixture main", "line": 2, "column": 1}]},
            "scopes": {"scopes": [{"name": "Locals", "variablesReference": 11}]},
            "variables": {"variables": [{"name": "ticket", "value": "42"}]},
        }.get(command, {})
        send(dict(type="response", seq=99, request_seq=incoming["seq"], success=True, command=command, body=body))
        if command == "configurationDone":
            event("stopped", {"threadId": 1, "reason": "breakpoint"})
        if command == "continue":
            event("continued", {"threadId": 1})
    elif mode == "lsp":
        method = incoming.get("method")
        params = incoming.get("params", {})
        with proof.open("a") as log:
            log.write(f"{method}\n")
        if method == "textDocument/didOpen":
            document = params["textDocument"]
            assert document["version"] == 1 and "new-greeting" in document["text"]
            send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
                  "params": {"uri": document["uri"], "version": 1, "diagnostics": [
                      {"range": {"start": {"line": 3, "character": 9}, "end": {"line": 3, "character": 12}},
                       "severity": 2, "message": "synthetic warning"}]}})
        if "id" in incoming:
            result = {
                "initialize": {"capabilities": {"positionEncoding": "utf-16"}},
                "textDocument/hover": {"contents": {"kind": "plaintext", "value": "synthetic hover"}},
                "textDocument/formatting": [{"range": {"start": {"line": 0, "character": 0},
                                                       "end": {"line": 0, "character": 0}},
                                             "newText": "// verified LSP\n"}],
            }.get(method)
            send({"jsonrpc": "2.0", "id": incoming["id"], "result": result})
