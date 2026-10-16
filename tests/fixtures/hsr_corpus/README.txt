Frozen copy of README.md, www/llms.txt and docs/**/*.md as of origin/main 62fd8176
(mcphost 0.108.42), one file per page, path + ".fixture". tests/hsr_ac01_* indexes
these instead of the live tree so its top-3 score cannot move when a branch edits
docs/. Do not refresh it to chase a green run; refresh only deliberately, when the
search ranking or the fixture rows change, and re-run hsr_ac01.
