# expected: string_arg_subscripted
import json


def main(args):
    payload = args.get("payload", "")
    rows = []
    for key in ("id", "name"):
        rows.append(key)
    ident = payload["id"]
    return {"id": ident, "rows": rows}
