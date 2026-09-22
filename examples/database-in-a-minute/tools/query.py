import mcphost

ALLOWED_OPS = {"=", "!=", "<", ">", "<=", ">="}


def _format_value(value):
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, (int, float)):
        return repr(value)
    if isinstance(value, str):
        return "'" + value.replace("'", "''") + "'"
    raise ValueError(f"unsupported where value type: {type(value).__name__}")


def main(args):
    table = args.get("table", "expenses")
    where = args.get("where")
    if where is None:
        where = []
    # Requirement 3 / AC4: `where` must be the structured
    # `[{col, op, value}]` shape -- a raw SQL string (or anything else
    # that isn't a list) is rejected here, before it ever reaches
    # `mcphost.state.query`.
    if not isinstance(where, list):
        raise ValueError(
            "where must be a list of {col, op, value} objects, not a raw SQL string"
        )

    clauses = []
    for clause in where:
        if not isinstance(clause, dict):
            raise ValueError("each where clause must be an object with col, op, value")
        col = clause.get("col")
        if not isinstance(col, str) or not col:
            raise ValueError("each where clause needs a string 'col'")
        op = clause.get("op")
        if op not in ALLOWED_OPS:
            raise ValueError(
                f"unsupported operator {op!r}; expected one of = != < > <= >= "
                "(LIKE and raw SQL are not allowed)"
            )
        if "value" not in clause:
            raise ValueError("each where clause needs a 'value'")
        clauses.append(f"{col} {op} {_format_value(clause['value'])}")

    limit = args.get("limit", 500)
    try:
        limit = int(limit)
    except (TypeError, ValueError):
        raise ValueError("limit must be an integer")
    limit = max(1, min(limit, 500))

    # No `limit` passed to the host call: the host's own `where` truncates
    # nothing on its own, so `count` below reflects every matching row,
    # not just the ones this tool ends up returning.
    result = mcphost.state.query(table=table, where=" and ".join(clauses) or None)
    rows = result["rows"]
    count = len(rows)
    rows = rows[:limit]

    columns = args.get("columns")
    if columns is not None:
        if not isinstance(columns, list) or not all(isinstance(c, str) for c in columns):
            raise ValueError("columns must be a list of strings")
        rows = [{c: r.get(c) for c in columns} for r in rows]

    return {"rows": rows, "count": count}
