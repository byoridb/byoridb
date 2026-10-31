#!/usr/bin/env python3
"""Run the real TLS deployment in a disposable Compose project (Docker required)."""
import argparse
import hashlib
import http.client
import json
import os
from pathlib import Path
import secrets
import socket
import ssl
import struct
import subprocess
import tempfile
import time
from rotate_cert import install_certificate

ROOT = Path(__file__).resolve().parents[2]
COMPOSE = Path(__file__).with_name("compose.yml")


def run(args, env=None, data=None):
    result = subprocess.run(args, input=data, capture_output=True, env=env)
    if result.returncode:
        # Request bodies are stdin, never argv or stderr. Scrub environment
        # secrets from tooling diagnostics; never dump container inspect/logs.
        detail = result.stderr.decode(errors="replace")[-4096:]
        for name, value in (env or {}).items():
            if value and any(word in name.upper() for word in ["PASSWORD", "TOKEN", "SECRET"]):
                detail = detail.replace(value, "[redacted]")
        raise RuntimeError(f"{args[0]} {args[1] if len(args) > 1 else ''} failed (exit {result.returncode}): {detail}")
    return result.stdout


def unused_port():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


def request(port, context, path, body=None, method=None):
    connection = http.client.HTTPSConnection("localhost", port, context=context, timeout=10)
    try:
        connection.request(method or ("POST" if body is not None else "GET"), path,
                           json.dumps(body).encode() if body is not None else None,
                           {"Content-Type": "application/json"})
        response = connection.getresponse()
        return response.status, response.read()
    finally:
        connection.close()


def varint(value):
    result = bytearray()
    while value >= 128:
        result.append((value & 127) | 128)
        value >>= 7
    result.append(value)
    return bytes(result)


def field(number, value):
    if isinstance(value, int):
        return varint(number << 3) + varint(value)
    value = value.encode() if isinstance(value, str) else value
    return varint((number << 3) | 2) + varint(len(value)) + value


def decode_fields(message):
    offset = 0
    result = {}
    def read_varint():
        nonlocal offset
        value, shift = 0, 0
        while True:
            byte = message[offset]
            offset += 1
            value |= (byte & 127) << shift
            if not byte & 128:
                return value
            shift += 7
            if shift > 70:
                raise AssertionError("Malformed protobuf varint")
    while offset < len(message):
        tag = read_varint()
        number, wire = tag >> 3, tag & 7
        if wire == 0:
            result[number] = read_varint()
        elif wire == 2:
            size = read_varint()
            result[number] = message[offset:offset + size]
            offset += size
        else:
            raise AssertionError(f"Unexpected protobuf wire type {wire}")
    return result


def grpc(port, ca, method, message):
    # curl/nghttp2 sends genuine HTTP/2 gRPC; no generated-client dependency.
    framed = b"\x00" + struct.pack(">I", len(message)) + message
    with tempfile.NamedTemporaryFile() as headers:
        response = run(["curl", "--http2", "--silent", "--show-error", "--fail", "--max-time", "15",
                        "--cacert", str(ca), "--header", "content-type: application/grpc",
                        "--header", "te: trailers", "--data-binary", "@-", "--dump-header", headers.name,
                        f"https://localhost:{port}/graph.GraphService/{method}"], data=framed)
        metadata = Path(headers.name).read_text().lower()
    assert "http/2 200" in metadata, "gRPC must negotiate HTTP/2 over TLS"
    assert "grpc-status: 0" in metadata, "gRPC transport rejected the request"
    assert len(response) >= 5 and response[0] == 0
    size = struct.unpack(">I", response[1:5])[0]
    assert size == len(response) - 5, "expected one complete unary gRPC frame"
    return decode_fields(response[5:])


def certificate_fingerprint(port, context):
    with socket.create_connection(("127.0.0.1", port), timeout=5) as connection:
        with context.wrap_socket(connection, server_hostname="localhost") as tls:
            return hashlib.sha256(tls.getpeercert(binary_form=True)).hexdigest()


def make_leaf(directory, ca, ca_key, suffix):
    key, csr, cert = [directory / (suffix + extension) for extension in [".key", ".csr", ".pem"]]
    run(["openssl", "req", "-new", "-newkey", "rsa:2048", "-nodes", "-subj", "/CN=localhost",
         "-keyout", str(key), "-out", str(csr)])
    extensions = directory / "leaf.ext"
    extensions.write_text("subjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\nextendedKeyUsage=serverAuth\nkeyUsage=critical,digitalSignature,keyEncipherment\nsubjectKeyIdentifier=hash\nauthorityKeyIdentifier=keyid,issuer\n")
    run(["openssl", "x509", "-req", "-in", str(csr), "-CA", str(ca), "-CAkey", str(ca_key),
         "-set_serial", str(secrets.randbits(120) + 1), "-days", "1", "-extfile", str(extensions), "-out", str(cert)])
    return cert, key


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build", action="store_true", help="Build the source Dockerfile before testing")
    parser.add_argument("--image", default="byoridb-server:local", help="Existing image, or tag for --build")
    args = parser.parse_args()
    run(["docker", "info", "--format", "{{.ServerVersion}}"])
    if args.build:
        print("Building the source container with Cargo.lock...", flush=True)
        # Build output contains no request data; leave progress visible for CI.
        subprocess.run(["docker", "build", "--tag", args.image, str(ROOT)], check=True)
    project = "byoridb-smoke-" + secrets.token_hex(6)
    compose = ["docker", "compose", "--file", str(COMPOSE), "--project-name", project]
    password, marker = "smoke-" + secrets.token_hex(24), "private_body_" + secrets.token_hex(12)
    http_port, grpc_port = unused_port(), unused_port()
    while grpc_port == http_port:
        grpc_port = unused_port()
    with tempfile.TemporaryDirectory(prefix="byoridb-tls-smoke-", dir="/tmp") as directory:
        directory = Path(directory).resolve()
        ca, ca_key, cert_dir = directory / "ca.pem", directory / "ca.key", directory / "certs"
        run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1", "-subj", "/CN=ByoriDB smoke CA",
             "-addext", "basicConstraints=critical,CA:TRUE", "-addext", "keyUsage=critical,keyCertSign,cRLSign",
             "-keyout", str(ca_key), "-out", str(ca)])
        cert, key = make_leaf(directory, ca, ca_key, "first")
        install_certificate(cert_dir, cert, key)
        context = ssl.create_default_context(cafile=str(ca))
        env = os.environ.copy()
        env.update(BYORIDB_ROOT_PASSWORD=password, BYORIDB_IMAGE=args.image,
                   BYORIDB_TLS_CERT_DIR=str(cert_dir), BYORIDB_TLS_CERT_GID=str(os.getgid()),
                   BYORIDB_TLS_BIND="127.0.0.1", BYORIDB_HTTPS_PORT=str(http_port), BYORIDB_GRPCS_PORT=str(grpc_port))
        started = False
        try:
            run(compose + ["config", "--quiet"], env)
            started = True
            try:
                run(compose + ["up", "--detach", "--no-build", "--wait", "--wait-timeout", "120"], env)
            except RuntimeError:
                # Syntax/permission diagnostics contain no request material.
                run(compose + ["run", "--rm", "--no-deps", "proxy", "-t", "-c", "/etc/nginx/nginx.conf"], env)
                raise
            for service in ["server", "proxy"]:
                container = run(compose + ["ps", "--quiet", service], env).decode().strip()
                inspected = json.loads(run(["docker", "inspect", container]))[0]
                assert inspected["Config"]["User"] == "10001:10001"
                assert inspected["HostConfig"]["ReadonlyRootfs"]
                assert "ALL" in inspected["HostConfig"]["CapDrop"]
                if service == "server":
                    assert not inspected["HostConfig"]["PortBindings"], "backend must not publish host ports"
            assert run(compose + ["exec", "-T", "server", "stat", "-c", "%u:%g", "/app/data"], env).strip() == b"10001:10001"
            for binary in ["byoridb-server", "byoridb-backup", "byoridb-cli", "byoridb-bulkloader"]:
                run(compose + ["exec", "-T", "server", binary, "--help" if binary == "byoridb-bulkloader" else "--version"], env)
            for endpoint in ["/health", "/ready", "/metrics", "/api/v1/metrics", "/api/v1/diagnostics/queries", "/unknown"]:
                assert request(http_port, context, endpoint)[0] == 403, f"ops route exposed: {endpoint}"
            for endpoint in ["/metrics", "/graph.GraphService/Unknown"]:
                assert request(grpc_port, context, endpoint)[0] == 403
            assert grpc(grpc_port, ca, "ExecuteJson", field(1, 0) + field(2, "SHOW SPACES")).get(1) == 2
            assert request(http_port, context, "/api/v1/query", {"session_id": "0", "query": "SHOW SPACES"})[0] == 401
            status, body = request(http_port, context, "/api/v1/session", {"username": "root", "password": password})
            assert status == 200
            session = json.loads(body)["session_id"]
            for query in ["CREATE SPACE secure_smoke", "USE secure_smoke", "CREATE TAG note(body STRING)", f'INSERT VERTEX note(body) VALUES 1:("{marker}")']:
                assert request(http_port, context, "/api/v1/query", {"session_id": session, "query": query})[0] == 200
            status, body = request(http_port, context, "/api/v1/query", {"session_id": session, "query": "MATCH (n:note) RETURN n.note.body AS body", "read_only": True})
            assert status == 200 and json.loads(body)["results"] == [{"body": marker}]
            authenticated = grpc(grpc_port, ca, "Authenticate", field(1, "root") + field(2, password))
            assert authenticated.get(2, 0) == 0 and authenticated.get(1, 0) != 0
            grpc_session = authenticated[1]
            for query in ["USE secure_smoke", "MATCH (n:note) RETURN n.note.body AS body"]:
                answer = grpc(grpc_port, ca, "ExecuteJson", field(1, grpc_session) + field(2, query))
                assert answer.get(1, 0) == 0
            assert json.loads(answer[4]) == {"column_names": ["body"], "rows": [[{"String": marker}]]}
            # A fresh client without the smoke CA must refuse this server.
            try:
                request(http_port, ssl.create_default_context(), "/health")
            except ssl.SSLError:
                pass
            else:
                raise AssertionError("TLS certificate verification was bypassed")
            old_fingerprint = certificate_fingerprint(http_port, context)
            replacement, replacement_key = make_leaf(directory, ca, ca_key, "replacement")
            try:
                install_certificate(cert_dir, replacement, key, compose, env)
            except ValueError:
                pass
            else:
                raise AssertionError("Mismatched certificate/key was accepted")
            assert certificate_fingerprint(http_port, context) == old_fingerprint
            install_certificate(cert_dir, replacement, replacement_key, compose, env)
            for _ in range(50):
                if certificate_fingerprint(http_port, context) != old_fingerprint:
                    break
                time.sleep(0.1)
            else:
                raise AssertionError("TLS certificate did not rotate on a fresh HTTPS connection")
            assert certificate_fingerprint(grpc_port, context) != old_fingerprint
            assert request(http_port, context, "/api/v1/query", {"session_id": session, "query": "SHOW SPACES"})[0] == 200
            assert grpc(grpc_port, ca, "ExecuteJson", field(1, grpc_session) + field(2, "SHOW SPACES")).get(1, 0) == 0
            print("PASS: HTTPS/gRPC TLS queries, private routes, certificate rotation.", flush=True)
            # Restart the actual database process while keeping its named volume.
            # Authentication sessions are ephemeral; data and schema must persist.
            run(compose + ["restart", "--timeout", "300", "server"], env)
            for _ in range(120):
                status, body = request(http_port, context, "/api/v1/session", {"username": "root", "password": password})
                if status == 200:
                    break
                time.sleep(0.5)
            else:
                raise AssertionError("Server did not recover after restart")
            restarted_session = json.loads(body)["session_id"]
            for query in ["USE secure_smoke", "MATCH (n:note) RETURN n.note.body AS body"]:
                status, body = request(http_port, context, "/api/v1/query", {"session_id": restarted_session, "query": query})
                assert status == 200
            assert json.loads(body)["results"] == [{"body": marker}], "Named volume did not retain the exact query result"
            authenticated = grpc(grpc_port, ca, "Authenticate", field(1, "root") + field(2, password))
            assert authenticated.get(2, 0) == 0 and authenticated.get(1, 0) != 0
            restarted_grpc_session = authenticated[1]
            for query in ["USE secure_smoke", "MATCH (n:note) RETURN n.note.body AS body"]:
                answer = grpc(grpc_port, ca, "ExecuteJson", field(1, restarted_grpc_session) + field(2, query))
                assert answer.get(1, 0) == 0
            assert json.loads(answer[4]) == {"column_names": ["body"], "rows": [[{"String": marker}]]}
            logs = run(compose + ["logs", "--no-color"], env)
            for sensitive in [password, marker, str(session), str(grpc_session), str(restarted_session), str(restarted_grpc_session)]:
                assert sensitive.encode() not in logs, "credential or query body appeared in container logs"
            print("PASS: nonroot binaries + data after server restart; private backend; HTTPS/gRPC TLS auth and queries; blocked ops; certificate rotation; redacted logs.")
        finally:
            if started:
                run(compose + ["down", "--volumes", "--remove-orphans", "--timeout", "300"], env)


if __name__ == "__main__":
    main()
