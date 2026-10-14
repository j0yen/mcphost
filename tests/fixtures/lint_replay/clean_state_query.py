# expected: (none)
import mcphost


def main(args):
    q = args.get("q", "")
    rows = mcphost.state.query(table="t")["rows"]
    return {"q": q, "rows": rows}
