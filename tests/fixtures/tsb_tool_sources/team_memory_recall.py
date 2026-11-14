import mcphost


def main(args):
    query = str(args["query"])
    limit = args.get("limit", 50)
    try:
        limit = int(limit)
    except (TypeError, ValueError):
        limit = 50
    limit = max(1, min(limit, 50))
    escaped = query.replace("'", "''")
    sql = (
        'SELECT rowid AS id, "key", "text", tags, writer, at FROM memory '
        f"WHERE \"key\" LIKE '%{escaped}%' OR \"text\" LIKE '%{escaped}%' OR tags LIKE '%{escaped}%' "
        f"ORDER BY at DESC, rowid DESC LIMIT {limit}"
    )
    result = mcphost.table.query(sql=sql)
    rows = result["rows"]
    return {"rows": rows, "found": len(rows) > 0}
