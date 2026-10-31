#!/usr/bin/env python3
"""Install a certificate/key pair atomically and reload only the named TLS proxy."""
import argparse
import os
from pathlib import Path
import secrets
import shutil
import subprocess


def command(args, env=None):
    result = subprocess.run(args, stdin=subprocess.DEVNULL, capture_output=True, env=env)
    if result.returncode:
        raise RuntimeError(f"{args[0]} failed (exit {result.returncode}); certificate was not accepted")
    return result.stdout


def install_certificate(cert_dir, cert, key, compose=None, env=None):
    cert_dir, cert, key = Path(cert_dir).resolve(), Path(cert).resolve(), Path(key).resolve()
    cert_public = command(["openssl", "x509", "-in", str(cert), "-pubkey", "-noout"])
    key_public = command(["openssl", "pkey", "-in", str(key), "-pubout"])
    if cert_public != key_public:
        raise ValueError("Certificate and private key do not match")
    command(["openssl", "x509", "-in", str(cert), "-checkend", "0", "-noout"])
    cert_dir.mkdir(mode=0o750, parents=True, exist_ok=True)
    cert_dir.chmod(0o750)
    group = cert_dir.stat().st_gid
    version = cert_dir / ("cert-" + secrets.token_hex(8))
    version.mkdir(mode=0o750)
    os.chown(version, -1, group)
    for source, name in [(cert, "fullchain.pem"), (key, "privkey.pem")]:
        destination = version / name
        shutil.copyfile(source, destination)
        os.chown(destination, -1, group)
        destination.chmod(0o640)
    # A regular include file is portable across Linux and Docker Desktop bind
    # mounts; replacing a symlink can leave Desktop guests seeing its old inode.
    active = cert_dir / "active.conf"
    if active.is_symlink() or (active.exists() and not active.is_file()):
        raise ValueError("TLS active.conf must be a regular file managed by this installer")
    previous = active.read_bytes() if active.exists() else None
    configuration = (f"ssl_certificate /etc/byoridb/tls/{version.name}/fullchain.pem;\n"
                     f"ssl_certificate_key /etc/byoridb/tls/{version.name}/privkey.pem;\n").encode()

    def publish(contents):
        temporary = cert_dir / (".active-" + secrets.token_hex(8))
        try:
            with temporary.open("xb") as output:
                os.fchmod(output.fileno(), 0o640)
                os.fchown(output.fileno(), -1, group)
                output.write(contents)
                output.flush()
                os.fsync(output.fileno())
            os.replace(temporary, active)
        finally:
            temporary.unlink(missing_ok=True)

    publish(configuration)
    if compose:
        try:
            mounted = command(compose + ["exec", "-T", "proxy", "cat", "/etc/byoridb/tls/active.conf"], env)
            if mounted != configuration:
                raise RuntimeError("The proxy cannot see the replacement certificate configuration")
            command(compose + ["exec", "-T", "proxy", "nginx", "-t", "-c", "/etc/nginx/nginx.conf"], env)
            command(compose + ["exec", "-T", "proxy", "nginx", "-s", "reload", "-c", "/etc/nginx/nginx.conf"], env)
        except Exception:
            if previous is None:
                active.unlink()
            else:
                publish(previous)
                # A failed reload may have taken effect before returning. Restore
                # the old pair on disk and try to reapply it; retain both versions.
                try:
                    command(compose + ["exec", "-T", "proxy", "nginx", "-s", "reload", "-c", "/etc/nginx/nginx.conf"], env)
                except RuntimeError:
                    pass
            raise
    return version


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cert-dir", type=Path, required=True)
    parser.add_argument("--cert", type=Path, required=True)
    parser.add_argument("--key", type=Path, required=True)
    parser.add_argument("--project", help="Existing Compose project to reload")
    parser.add_argument("--no-reload", action="store_true", help="Initial install before starting the proxy")
    args = parser.parse_args()
    if not args.no_reload and not args.project:
        parser.error("--project is required when reloading a running proxy")
    compose = None if args.no_reload else ["docker", "compose", "-f", str(Path(__file__).with_name("compose.yml")), "--project-name", args.project]
    installed = install_certificate(args.cert_dir, args.cert, args.key, compose)
    print(f"Certificate installed: {installed.name}" + (" (proxy reload requested)" if compose else ""))


if __name__ == "__main__":
    main()
