import mcphost

NOTES_TABLE = "quickstart_notes"


def main(args):
    note = args.get("note", "")
    if not isinstance(note, str) or not note:
        return {"error": "note must be a non-empty string"}
    try:
        mcphost.table.create(name=NOTES_TABLE, columns={"note": "text"})
    except mcphost.table.TableError as e:
        if e.code != "table_already_exists":
            raise
    result = mcphost.table.append(table=NOTES_TABLE, rows=[{"note": note}])
    return {"appended": result["appended"], "table": NOTES_TABLE}
