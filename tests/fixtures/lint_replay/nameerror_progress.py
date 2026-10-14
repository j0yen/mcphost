# expected: mcphost_not_imported
import time


def main(args):
    total = int(args.get("n", 3))
    for i in range(total):
        mcphost.progress(i * 100 // total, "working")
        time.sleep(0)
    return {"done": total}
