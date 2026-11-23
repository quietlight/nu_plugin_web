# nu_plugin_web

Web search and page reading as nushell commands, through
[Parallel](https://docs.parallel.ai)'s Search and Extract APIs:

```nu
web search 'How is kauri dieback spread and controlled?' 'kauri dieback spread' 'kauri dieback control'
#  → table<url, title, publish_date, excerpts>
web search 'nushell release notes' --include [nushell.sh] -n 2 | web extract --full
#  → table<url, title, publish_date, excerpts, full_content>
```

`web search` takes an objective (a sentence: what you want to learn) and
optional short keyword queries; with no queries the objective is the query.
`web extract` takes URLs as arguments or piped in (strings, or rows with a
`url` column), with an optional `--objective` to focus the excerpts and
`--full` for whole pages. Pages that fail come back as rows with an `error`
column. `--raw` on either returns the API reply unshaped.

## A narrow capability

The plugin exists so an agent's nu environment can search and read the web
without being handed `http get`. It can only:

- reach `https://api.parallel.ai/v1/search` and `/v1/extract` (compiled in,
  no URL argument); the rest of Parallel (tasks, FindAll, monitors, chat) is
  not exposed;
- return at most 10 results and 100k characters per call (defaults: 5
  results, 20k characters), at most 20 URLs per extract;
- send a request body of at most 16 KiB, with a 60 s timeout.

Default search mode is `fast` ($1 per 1k requests); `--mode basic|advanced`
costs $5 per 1k. Extract is $1 per 1k URLs.

The key is read from `~/.config/parallel/api_key`, falling back to
`$env.PARALLEL_API_KEY`. Prefer the file: an agent's script can read `$env`,
and the file can be kept off the agent's readable paths.

Queries and URLs leave the machine, and Parallel then fetches the URLs it is
given, so a URL is also a way to send data out. This narrows where data goes,
not whether it goes: list the plugin only in profiles that need the web.

## Install

Needs Rust and nushell 0.117.x. The plugin protocol is locked to nu's minor
version, so the plugin's `nu-plugin`/`nu-protocol` versions must match your nu.

```nu
cargo install --git https://github.com/quietlight/nu_plugin_web --locked
plugin add ~/.cargo/bin/nu_plugin_web
plugin use web
```

From a checkout, `cargo install --path . --locked` does the same. For op, list
`web` in a profile's `plugins`: op finds `nu_plugin_web` on `PATH` and registers it
itself.

## License

MIT. See [LICENSE](LICENSE).
