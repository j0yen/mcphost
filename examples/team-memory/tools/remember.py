import mcphost
import time


def main(args):
    key = args["key"]
    text = args["text"]
    tags = args.get("tags") or []
    who = args.get("who")
    if not who:
        raise ValueError(
            "who is required: mcphost does not expose the calling tenant's "
            "own id inside a shared python tool yet, so remember trusts "
            "whatever tenant namespace the caller states itself (see "
            "www/llms.txt's 'Give your agents one memory' section)"
        )
    result = mcphost.table.append(
        table="memory",
        rows=[{"key": key, "text": text, "tags": tags, "writer": who, "at": time.time()}],
    )
    return {"id": result["ids"][0], "key": key, "writer": who}
