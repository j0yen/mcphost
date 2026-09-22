import mcphost

# Replaced with the owner's own tenant namespace before this source is
# published (see proof.sh / the AC9 test) -- baked in at publish time
# since a shared tool has no other way to learn its own owner's identity
# from inside the sandbox (see remember.py's `who` note).
OWNER_NAMESPACE = "__OWNER_NAMESPACE__"


def main(args):
    row_id = int(args["id"])
    who = args.get("who")
    rows = mcphost.table.query(sql=f"SELECT writer FROM memory WHERE rowid = {row_id}")["rows"]
    if not rows:
        raise ValueError(f"no such memory row: {row_id}")
    writer = rows[0]["writer"]
    if who != writer and who != OWNER_NAMESPACE:
        raise PermissionError(
            f"forget refused: {who} is neither the row's writer ({writer}) nor the owner"
        )
    # mcphost.table has no row-level delete (create/append/query/list/drop/
    # schema only) -- forgetting is recorded as a tombstone instead, the
    # same append-only primitive remember itself uses.
    try:
        mcphost.table.create(name="memory_forgotten", columns={"id": "integer"})
    except mcphost.table.TableError as e:
        if e.code != "table_already_exists":
            raise
    mcphost.table.append(table="memory_forgotten", rows=[{"id": row_id}])
    return {"ok": True, "forgotten": row_id}
