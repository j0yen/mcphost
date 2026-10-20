"""Consumes tests/fixtures/audit_vectors.json, which tests/audx_ac04_*.rs
generates from the Rust canonical form, chain and signature functions.

    python3 -I tests/test_audit_vectors.py            # the committed file
    AUDIT_VECTORS=/path/to/copy.json python3 -I tests/test_audit_vectors.py
"""
import importlib.util
import json
import os
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("audit_verify", os.path.join(HERE, "..", "scripts", "audit-verify.py"))
av = importlib.util.module_from_spec(spec)
spec.loader.exec_module(av)

with open(os.environ.get("AUDIT_VECTORS", os.path.join(HERE, "fixtures", "audit_vectors.json")), encoding="utf-8") as f:
    V = json.load(f)


class AuditVectors(unittest.TestCase):
    def test_five_lines(self):
        self.assertEqual(len(V["lines"]), 5)

    def test_every_vector_line_recomputes(self):
        prev = V["genesis"]
        for i, (line, canon) in enumerate(zip(V["lines"], V["canonical"]), start=1):
            body = {k: v for k, v in line.items() if k != "hash"}
            self.assertEqual(av.canonical(body), canon, f"canonical form of vector {i}")
            self.assertEqual(line["prev"], prev, f"prev of vector {i}")
            self.assertEqual(av.line_hash(prev, body), line["hash"], f"hash of vector {i}")
            prev = line["hash"]
        self.assertEqual(prev, V["root_hash"])

    def test_signature_verifies_against_the_vector_jwks(self):
        t = V["trailer"]
        self.assertEqual(t["root_hash"], V["root_hash"])
        self.assertEqual(V["signed_message"], f"{t['root_hash']}{t['rows']}")
        self.assertTrue(av.signature_ok(V["jwks"], t["kid"], t["root_hash"], t["rows"], t["signature"]))

    def test_the_assembled_export_verifies_end_to_end(self):
        text = "".join(av.canonical(o) + "\n" for o in [V["header"], *V["lines"], V["trailer"]])
        ok, message = av.verify(text, V["jwks"])
        self.assertTrue(ok, message)
        self.assertTrue(message.startswith("OK 5 rows"), message)


if __name__ == "__main__":
    unittest.main()
