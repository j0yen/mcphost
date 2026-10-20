# Parity fixture for PRD-mcphost-dev-emulator: the error is raised at line 14.
# Keep the raising statement on line 14; the parity test asserts the line
# number and the source text.
#
# The rows are a list; indexing one with a string key is the TypeError an
# agent sees on prod.
#
#
#
#
def main(args):
    rows = args.get("rows") or ["a", "b"]
    first = rows[0]
    return rows["name"]
