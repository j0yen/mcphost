# expected: return_not_json
import requests


def main(args):
    url = args.get("url", "https://example.com/api")
    resp = requests.get(url, timeout=10)
    resp.raise_for_status()
    return resp
