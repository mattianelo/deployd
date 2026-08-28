#!/usr/bin/env python3
import json
import os
import selectors
import subprocess
import sys
import time
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
ANALYSIS_ROOT = os.environ.get("DEPLOYD_MCP_WORKSPACE", "/workspace")


class McpSession:
    def __init__(self, wrapper: str):
        env_name = f"DEPLOYD_MCP_{wrapper.removesuffix('.sh').replace('-', '_').upper()}_WRAPPER"
        command = os.environ.get(env_name)
        wrapper_path = Path(command) if command else ROOT / "scripts" / "mcp" / wrapper
        self.process = subprocess.Popen(
            [str(wrapper_path)],
            cwd=ROOT,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
        )
        if self.process.stdin is None or self.process.stdout is None:
            raise RuntimeError(f"failed to open {wrapper} protocol pipes")
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.process.stdout, selectors.EVENT_READ)
        self.next_id = 1

    def notify(self, method: str, params: dict) -> None:
        self._write({"jsonrpc": "2.0", "method": method, "params": params})

    def call(self, method: str, params: dict, timeout: int = 120) -> dict:
        request_id = self.next_id
        self.next_id += 1
        self._write(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "method": method,
                "params": params,
            }
        )

        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            remaining = deadline - time.monotonic()
            if not self.selector.select(remaining):
                break
            line = self.process.stdout.readline()
            if not line:
                break
            response = json.loads(line)
            if response.get("id") == request_id:
                if "error" in response:
                    raise RuntimeError(f"MCP error: {response['error']}")
                return response["result"]

        raise TimeoutError(f"timed out waiting for MCP request {request_id}")

    def close(self) -> None:
        self.process.stdin.close()
        try:
            return_code = self.process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.process.terminate()
            return_code = self.process.wait(timeout=10)
        if return_code != 0:
            raise RuntimeError(f"MCP wrapper exited with status {return_code}")

    def _write(self, message: dict) -> None:
        self.process.stdin.write(json.dumps(message) + "\n")
        self.process.stdin.flush()


def initialize(session: McpSession, expected_name: str) -> None:
    result = session.call(
        "initialize",
        {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "deployd-smoke", "version": "1"},
        },
    )
    if result["serverInfo"]["name"] != expected_name:
        raise RuntimeError(f"unexpected server identity: {result['serverInfo']}")
    session.notify("notifications/initialized", {})


def tool_text(session: McpSession, name: str, arguments: dict) -> str:
    result = session.call("tools/call", {"name": name, "arguments": arguments})
    if result.get("isError"):
        raise RuntimeError(result["content"][0]["text"])
    return result["content"][0]["text"]


def finds_is_newer_position() -> tuple[int, int]:
    lines = (ROOT / "src" / "core" / "update_check.rs").read_text().splitlines()
    for line_number, line in enumerate(lines):
        character = line.find("is_newer")
        if character >= 0 and line.lstrip().startswith("fn is_newer"):
            return line_number, character
    raise RuntimeError("could not locate is_newer")


def rust_analyzer_returns_semantic_results() -> None:
    session = McpSession("rust-analyzer.sh")
    try:
        initialize(session, "rust-analyzer-mcp")
        file_path = f"{ANALYSIS_ROOT}/src/core/update_check.rs"
        symbols = tool_text(
            session,
            "rust_analyzer_symbols",
            {"file_path": file_path},
        )
        if "is_newer" not in symbols:
            raise RuntimeError("rust-analyzer symbol result omitted is_newer")

        diagnostics = json.loads(
            tool_text(
                session,
                "rust_analyzer_diagnostics",
                {"file_path": file_path},
            )
        )
        if diagnostics["summary"]["errors"] != 0:
            raise RuntimeError("rust-analyzer reported errors in update_check.rs")

        line, character = finds_is_newer_position()
        position = {"file_path": file_path, "line": line, "character": character}
        if "fn is_newer" not in tool_text(session, "rust_analyzer_hover", position):
            raise RuntimeError("rust-analyzer hover result omitted is_newer")
        if "update_check.rs" not in tool_text(
            session, "rust_analyzer_references", position
        ):
            raise RuntimeError("rust-analyzer reference result omitted update_check.rs")
    finally:
        session.close()


def fossil_returns_structural_results() -> None:
    session = McpSession("fossil.sh")
    try:
        initialize(session, "fossil-mcp")
        scan = json.loads(tool_text(session, "scan_all", {"path": ANALYSIS_ROOT}))
        if not scan["analyses"]:
            raise RuntimeError("Fossil full scan returned no analyses")
        inspected = json.loads(
            tool_text(
                session,
                "fossil_inspect",
                {
                    "mode": "call_graph",
                    "function_name": "is_newer",
                    "path": ANALYSIS_ROOT,
                    "depth": 2,
                },
            )
        )
        if inspected["function"] != "is_newer":
            raise RuntimeError("Fossil inspected an unexpected function")
        traced = json.loads(
            tool_text(
                session,
                "fossil_trace",
                {
                    "from_function": "is_newer",
                    "to_function": "parse_semver",
                    "path": ANALYSIS_ROOT,
                    "max_depth": 5,
                    "max_paths": 3,
                },
            )
        )
        if not traced["connected"]:
            raise RuntimeError("Fossil did not trace is_newer to parse_semver")
    finally:
        session.close()


def main() -> int:
    if os.geteuid() == 0:
        raise RuntimeError("MCP smoke tests must not run as root")
    before = subprocess.check_output(
        ["git", "status", "--porcelain=v1", "--untracked-files=all"],
        cwd=ROOT,
        text=True,
    )
    rust_analyzer_returns_semantic_results()
    fossil_returns_structural_results()
    after = subprocess.check_output(
        ["git", "status", "--porcelain=v1", "--untracked-files=all"],
        cwd=ROOT,
        text=True,
    )
    if before != after or (ROOT / ".fossil").exists():
        raise RuntimeError("MCP smoke tests changed the checkout")
    print("MCP protocol smoke tests passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
