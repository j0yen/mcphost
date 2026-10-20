#!/usr/bin/env python3
"""Verify a `host.audit.export` JSONL with no mcphost account.

    python3 audit-verify.py export.jsonl --jwks https://mcphost.dev/.well-known/jwks.json

Needs only the stdlib and `cryptography`. Prints `OK <rows> rows root <hash>`
and exits 0, or `FAIL line <n>` / `FAIL signature` / `FAIL <what>` and exits 1.
`<n>` is the physical line of the file (the header is line 1).

The algorithm, pinned by tests/fixtures/audit_vectors.json (generated from the
Rust functions, so this file cannot drift from them):
  canonical(obj) = compact JSON, keys sorted at every depth, UTF-8 left as is
  hash           = sha256_hex(prev + canonical(line without "hash"))
  line 2's prev  = GENESIS; every later prev is the line above's hash
  root_hash      = last line's hash (GENESIS for zero rows)
  signature      = ES256 (base64url r||s) over root_hash + str(rows), key by kid
"""
import base64
import hashlib
import json
import sys
import urllib.request

GENESIS = "0" * 64


def canonical(obj):
    return json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def line_hash(prev, body):
    return hashlib.sha256((prev + canonical(body)).encode("utf-8")).hexdigest()


def b64url(s):
    return base64.urlsafe_b64decode(s + "=" * (-len(s) % 4))


def load_jwks(src):
    if src.startswith(("http://", "https://")):
        with urllib.request.urlopen(src, timeout=30) as r:
            return json.load(r)
    with open(src, encoding="utf-8") as f:
        return json.load(f)


def signature_ok(jwks, kid, root_hash, rows, signature):
    from cryptography.exceptions import InvalidSignature
    from cryptography.hazmat.primitives import hashes
    from cryptography.hazmat.primitives.asymmetric import ec
    from cryptography.hazmat.primitives.asymmetric.utils import encode_dss_signature

    try:
        jwk = next(k for k in jwks["keys"] if k.get("kid") == kid)
        raw = b64url(signature)
        if len(raw) != 64:
            return False
        point = ec.EllipticCurvePublicNumbers(
            int.from_bytes(b64url(jwk["x"]), "big"), int.from_bytes(b64url(jwk["y"]), "big"), ec.SECP256R1()
        )
        der = encode_dss_signature(int.from_bytes(raw[:32], "big"), int.from_bytes(raw[32:], "big"))
        point.public_key().verify(der, f"{root_hash}{rows}".encode("utf-8"), ec.ECDSA(hashes.SHA256()))
        return True
    except (InvalidSignature, StopIteration, KeyError, ValueError):
        return False


def verify(text, jwks):
    """Returns (ok, message)."""
    lines = text.split("\n")
    if lines and lines[-1] == "":
        lines.pop()
    if len(lines) < 2:
        return False, "FAIL line 1"
    try:
        header = json.loads(lines[0])
        trailer = json.loads(lines[-1])
    except ValueError:
        return False, "FAIL line 1" if not lines[0].startswith("{") else f"FAIL line {len(lines)}"
    prev, last_key = GENESIS, None
    for n, raw in enumerate(lines[1:-1], start=2):
        try:
            obj = json.loads(raw)
            claimed = obj.pop("hash")
            key = (obj["ts"], obj["table"], obj["id"])
            if obj["prev"] != prev or line_hash(prev, obj) != claimed or (last_key and key < last_key):
                return False, f"FAIL line {n}"
        except (ValueError, KeyError, TypeError, AttributeError):
            return False, f"FAIL line {n}"
        prev, last_key = claimed, key
    rows = len(lines) - 2
    if trailer.get("root_hash") != prev or trailer.get("rows") != rows:
        return False, f"FAIL line {len(lines)}"
    if header.get("kid") != trailer.get("kid") or not signature_ok(
        jwks, trailer.get("kid"), prev, rows, trailer.get("signature", "")
    ):
        return False, "FAIL signature"
    return True, f"OK {rows} rows root {prev}"


def main(argv):
    args = [a for a in argv[1:] if not a.startswith("--")]
    jwks_src = argv[argv.index("--jwks") + 1] if "--jwks" in argv else None
    if jwks_src is not None:
        args.remove(jwks_src)
    if len(args) != 1 or jwks_src is None:
        print("usage: audit-verify.py export.jsonl --jwks <url-or-file>", file=sys.stderr)
        return 2
    with open(args[0], encoding="utf-8", newline="") as f:
        text = f.read()
    ok, message = verify(text, load_jwks(jwks_src))
    print(message)
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
