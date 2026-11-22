"""Create short-lived loopback TLS material for the delivery fixtures."""

import os
from ipaddress import IPv4Address
from pathlib import Path
import shutil
import subprocess


def find_openssl(env=None):
    openssl = shutil.which("openssl", path=None if env is None else env.get("PATH", os.defpath))
    if openssl is None and os.name == "nt":
        git_core = Path(subprocess.check_output(["git", "--exec-path"], text=True, timeout=10, env=env).strip())
        bundled = git_core.parents[2] / "usr" / "bin" / "openssl.exe"
        if bundled.is_file():
            openssl = str(bundled)
    if openssl is None:
        raise RuntimeError("OpenSSL is required for the shared queue fixture")
    return openssl


def create_tls_files(root, env=None, *, address="127.0.0.1"):
    address = IPv4Address(address)
    openssl = find_openssl(env)

    def generate(*arguments):
        completed = subprocess.run([openssl, *arguments], cwd=root, env=env,
                                   capture_output=True, text=True, timeout=15)
        if completed.returncode != 0:
            raise RuntimeError(f"Could not generate disposable TLS material: {completed.stderr.strip()}")

    (root / "server.ext").write_text(
        "basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\n"
        "extendedKeyUsage=serverAuth\nsubjectKeyIdentifier=hash\nauthorityKeyIdentifier=keyid\n"
        f"subjectAltName=IP:{address}\n", encoding="ascii")
    generate("req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1", "-subj", "/CN=Factory fixture CA",
             "-addext", "basicConstraints=critical,CA:TRUE", "-addext", "keyUsage=critical,keyCertSign,cRLSign",
             "-keyout", "ca.key", "-out", "ca.pem")
    generate("req", "-new", "-newkey", "rsa:2048", "-nodes", "-subj", f"/CN={address}",
             "-keyout", "server.key", "-out", "server.csr")
    generate("x509", "-req", "-in", "server.csr", "-CA", "ca.pem", "-CAkey", "ca.key", "-CAcreateserial",
             "-out", "server.pem", "-days", "1", "-extfile", "server.ext")
