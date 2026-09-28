import mcphost

# examples/docs-qa/ask_docs.py -- the recipe's one published tool. Calls
# `mcphost.docs.search` over the zero-network, zero-metered-call loopback
# channel (docs-qa.sh's own `host.docs.put` corpus is what this searches),
# and formats each passage with a `name:offset` citation so the answer is
# checkable against the source document. Requirement 1/AC1/AC8.


def main(args):
    query = args.get("query")
    if not query or not isinstance(query, str):
        raise ValueError("missing required argument 'query' (a string)")
    k = args.get("k", 5)

    result = mcphost.docs.search(query=query, k=k)
    hits = result.get("results") or []

    # AC8: zero passages is a structured result, not an error -- a caller
    # (or a persona scoring this tool) can branch on `found` without ever
    # seeing an exception.
    if not hits:
        return {
            "query": query,
            "found": False,
            "message": "no passages found",
            "passages": [],
            "index_mode": (result.get("index") or {}).get("mode"),
        }

    passages = [
        {
            "citation": f"{hit['name']}:{hit['offset']}",
            "name": hit["name"],
            "offset": hit["offset"],
            "text": hit["text"],
            "score": hit.get("score"),
        }
        for hit in hits
    ]
    return {
        "query": query,
        "found": True,
        "passages": passages,
        "index_mode": (result.get("index") or {}).get("mode"),
    }
