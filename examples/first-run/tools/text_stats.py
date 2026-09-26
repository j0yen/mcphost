def main(args):
    text = args.get("text", "")
    reversed_text = text[::-1]
    words = len(text.split())
    return {
        "reversed": reversed_text,
        "words": words,
    }
