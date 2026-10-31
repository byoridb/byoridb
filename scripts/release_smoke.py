#!/usr/bin/env python3
"""Exercise extracted release executables, including an offline restore.

Uses only Python's standard library; never touches an operator's data directory.
Every child process and network operation has a timeout. Secrets are passed by
environment and checked against the captured server logs before cleanup.
"""
import argparse
import json
import os
from pathlib import Path
import re
import secrets
import signal
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def run(binary, args, env, cwd):
    result = subprocess.run([str(binary), *args], env=env, cwd=cwd,
                            capture_output=True, text=True, timeout=90)
    require(result.returncode == 0,
            f"{binary.name} {args[0]} failed: {result.stderr}")
    return result.stdout


class Server:
    def __init__(self, binary, data, env, workspace):
        self.http_port = free_port()
        self.grpc_port = free_port()
        while self.grpc_port == self.http_port:
            self.grpc_port = free_port()
        self.log_path = workspace / f"server-{secrets.token_hex(4)}.log"
        self.log = self.log_path.open("w")
        self.env = dict(env, BYORIDB__STORAGE__DATA_PATHS=str(data),
                        BYORIDB__SERVER__GRAPH_ADDR=f"127.0.0.1:{self.grpc_port}",
                        BYORIDB__SERVER__HTTP_ADDR=f"127.0.0.1:{self.http_port}")
        self.child = subprocess.Popen([str(binary)], cwd=workspace, env=self.env,
                                      stdout=self.log, stderr=self.log)

    def request(self, route, payload=None):
        req = urllib.request.Request(
            f"http://127.0.0.1:{self.http_port}{route}",
            data=None if payload is None else json.dumps(payload).encode(),
            headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=5) as response:
            if payload is None:
                return response.read().decode()
            return json.load(response)

    def ready(self):
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            require(self.child.poll() is None, "server exited before readiness")
            try:
                require(self.request("/ready") == "READY", "unexpected readiness body")
                return
            except (OSError, urllib.error.URLError):
                time.sleep(0.1)
        raise RuntimeError("server readiness timed out")

    def login(self):
        return self.request("/api/v1/session", {
            "username": "root", "password": self.env["BYORIDB_ROOT_PASSWORD"]
        })["session_id"]

    def query(self, session, text, read_only=False):
        return self.request("/api/v1/query", {
            "session_id": session, "query": text, "read_only": read_only})

    def stop(self):
        require(self.child.poll() is None, "server exited unexpectedly")
        self.child.send_signal(signal.SIGTERM)
        try:
            code = self.child.wait(timeout=45)
        except subprocess.TimeoutExpired:
            self.child.kill()
            self.child.wait(timeout=10)
            raise RuntimeError("graceful shutdown timed out")
        finally:
            self.log.close()
        require(code == 0, f"graceful shutdown returned {code}")

    def cleanup(self):
        if self.child.poll() is None:
            self.child.kill()
            self.child.wait(timeout=10)
        self.log.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", type=Path, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--commit", help="Require an exact clean build commit")
    args = parser.parse_args()
    binaries = {name: args.bin_dir.resolve() / f"byoridb-{name}"
                for name in ("server", "cli", "backup")}
    env = {k: v for k, v in os.environ.items() if not k.startswith("BYORIDB")}
    secret = "release-smoke-" + secrets.token_hex(24)
    env.update(BYORIDB_ROOT_PASSWORD=secret, BYORIDB_PASSWORD=secret,
               BYORIDB_USER="root", RUST_LOG="info")
    servers = []
    timings = {}
    with tempfile.TemporaryDirectory(prefix="byoridb-release-smoke-") as tmp:
        work = Path(tmp)
        try:
            for name, binary in binaries.items():
                require(binary.is_file() and os.access(binary, os.X_OK),
                        f"missing executable: {binary.name}")
                version = run(binary, ["--version"], env, work).strip()
                require(re.match(rf"byoridb-{name} {re.escape(args.version)}(?:\s|$)", version),
                        f"unexpected version: {version}")
                if args.commit and name != "cli":
                    require(f"commit {args.commit[:12]}, release" in version,
                            f"build identity mismatch: {version}")
                run(binary, ["--help"], env, work)
            require(not (work / "data").exists(), "help/version created data")

            data = work / "database"
            unique_value = "persist-" + secrets.token_hex(12)
            expected = None
            for round_number, path in enumerate((data, data, work / "restored")):
                server = Server(binaries["server"], path, env, work)
                servers.append(server)
                started = time.monotonic()
                server.ready()
                timings[f"startup_{round_number}_seconds"] = round(time.monotonic() - started, 3)
                session = server.login()
                if round_number == 0:
                    for query in [
                        "CREATE SPACE release_smoke",
                        "USE release_smoke",
                        "CREATE TAG note(body STRING)",
                        "CREATE TAG INDEX note_body ON note(body)",
                        f'INSERT VERTEX note(body) VALUES 1:("{unique_value}")',
                    ]:
                        server.query(session, query)
                    try:
                        server.query(session, "DELETE VERTEX 1", read_only=True)
                        raise RuntimeError("read_only accepted mutation")
                    except urllib.error.HTTPError as error:
                        require(error.code == 403, "unexpected read_only error")
                    try:
                        server.request("/api/v1/query", {"session_id": session,
                            "query": "DELETE VERTEX 1", "params": {"unused": 1}})
                        raise RuntimeError("HTTP silently ignored unsupported parameters")
                    except urllib.error.HTTPError as error:
                        require(error.code == 422, "unexpected unsupported-parameter error")
                else:
                    server.query(session, "USE release_smoke")
                result = server.query(session,
                    f'LOOKUP ON note WHERE note.body == "{unique_value}" YIELD note.body')
                require(result["row_count"] == 1 and unique_value in json.dumps(result["results"]),
                        "durable indexed lookup did not return the written row")
                if expected is None:
                    expected = result["results"]
                require(result["results"] == expected, "restart/restore changed result")
                grpc = run(binaries["cli"], ["--addr", f"127.0.0.1:{server.grpc_port}",
                    "--execute", "USE release_smoke; MATCH (n:note) RETURN n.note.body"], env, work)
                require(unique_value in grpc, "gRPC query did not read persisted value")
                server.stop()
                if round_number == 1:
                    archives = work / "archives"
                    timings["database_bytes"] = (data / "data.redb").stat().st_size
                    started = time.monotonic()
                    run(binaries["backup"], ["create", "--db", str(data),
                        "--backup-dir", str(archives)], env, work)
                    timings["backup_seconds"] = round(time.monotonic() - started, 3)
                    manifests = list(archives.glob("backup_*/backup_metadata.json"))
                    require(len(manifests) == 1, "backup manifest missing")
                    backup_id = manifests[0].parent.name
                    started = time.monotonic()
                    run(binaries["backup"], ["verify", "--backup-dir", str(archives),
                        "--backup-id", backup_id], env, work)
                    timings["verify_seconds"] = round(time.monotonic() - started, 3)
                    started = time.monotonic()
                    run(binaries["backup"], ["restore", "--backup-dir", str(archives),
                        "--backup-id", backup_id, "--target", str(work / "restored")], env, work)
                    timings["restore_seconds"] = round(time.monotonic() - started, 3)
            for server in servers:
                log = server.log_path.read_text()
                require(secret not in log and unique_value not in log,
                        "server logged a credential or query value")
            print("PASS: versions/help, HTTP + gRPC, read-only, SIGTERM, durable restart, backup/restore, log redaction")
            print("Fixture timings (not a production RTO): " + json.dumps(timings, sort_keys=True))
        finally:
            for server in servers:
                server.cleanup()


if __name__ == "__main__":
    main()
