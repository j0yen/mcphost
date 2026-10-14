# expected: return_not_json
import datetime


def main(args):
    seen = set(args.get("tags", "").split(","))
    return seen
