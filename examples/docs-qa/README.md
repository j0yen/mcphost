Documents are text the host indexes itself; a vector index built elsewhere (LlamaIndex, Chroma, pgvector) is not importable: export its source documents and put them.

# Docs Q&A in a minute

`docs-qa.sh <endpoint>` turns the 8-document markdown corpus in `corpus/` into a
checkable Q&A tool against a real mcphost endpoint: one fresh tenant, the corpus
loaded with `host.docs.put_many`, the index awaited, `ask_docs` published, and
the 6 gold questions in `corpus/gold.json` asked (5 of 6 must hit). It writes a
receipt JSON (per-question hit and citation, `index_ready_secs`, the quota used).

The section of `www/llms.txt` named "Docs Q&A in a minute" walks the same calls.
