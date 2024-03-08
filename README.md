# pigi

Use GitHub release page artifact as python package index

# Running

Set env variable `REPOS_CONFIG_PATH` to path to your config for list of packages you want o support
if you want to proxy private repos without you can set `GITHUB_TOKEN` env variable to private token used 
with all communication with github

```bash
cargo run
```

# Using with poetry

Add source to poetry:

```bash
poetry source add --priority=supplemental pigi http://localhost:8000/simple/
poetry config http-basic.pigi username $GITHUB_PERSONAL_TOKEN
```