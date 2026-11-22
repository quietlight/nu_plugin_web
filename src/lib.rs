//! `nu_plugin_web`: web search and page extraction as nu commands, through
//! Parallel's Search and Extract APIs.
//!
//! ```nu
//! web search 'recent research on kauri dieback' 'kauri dieback 2026' 'Phytophthora agathidicida'
//! 'https://www.doc.govt.nz/...' | web extract --objective 'how is it spread'
//! ```
//!
//! The plugin is a narrow capability: two endpoints on one host, a few
//! knobs, capped sizes. The rest of Parallel (tasks, monitors, FindAll,
//! chat) is deliberately absent. See `api.rs` for the limits.

use nu_plugin::{EngineInterface, EvaluatedCall, Plugin, SimplePluginCommand};
use nu_protocol::{Category, Example, LabeledError, Signature, SyntaxShape, Type, Value};

mod api;

pub struct WebPlugin;

impl Plugin for WebPlugin {
    fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").into()
    }

    fn commands(&self) -> Vec<Box<dyn nu_plugin::PluginCommand<Plugin = Self>>> {
        vec![Box::new(Web), Box::new(WebSearch), Box::new(WebExtract)]
    }
}

fn category() -> Category {
    Category::Custom("web".into())
}

fn key(engine: &EngineInterface, call: &EvaluatedCall) -> Result<String, LabeledError> {
    api::api_key(engine.get_env_var(api::KEY_ENV)?, call.head)
}

fn strings(call: &EvaluatedCall, flag: &str) -> Result<Vec<String>, LabeledError> {
    Ok(call.get_flag::<Vec<String>>(flag)?.unwrap_or_default())
}

fn max_chars(call: &EvaluatedCall) -> Result<i64, LabeledError> {
    let n = call.get_flag("max-chars")?.unwrap_or(api::DEFAULT_CHARS);
    api::bounded("max-chars", n, api::MAX_CHARS, call.head)
}

// ── web ─────────────────────────────────────────────────────────────────────

pub struct Web;

impl SimplePluginCommand for Web {
    type Plugin = WebPlugin;

    fn name(&self) -> &str {
        "web"
    }

    fn description(&self) -> &str {
        "Search the web and read pages (Parallel Search and Extract APIs)."
    }

    fn extra_description(&self) -> &str {
        "Two commands: `web search` finds pages and returns ranked excerpts; \
         `web extract` reads given URLs. The plugin talks only to api.parallel.ai \
         and caps result counts and sizes. Queries and URLs leave the machine. \
         The key is read from ~/.config/parallel/api_key, else $env.PARALLEL_API_KEY."
    }

    fn signature(&self) -> Signature {
        Signature::build(self.name()).category(category())
    }

    fn run(
        &self,
        _plugin: &WebPlugin,
        engine: &EngineInterface,
        call: &EvaluatedCall,
        _input: &Value,
    ) -> Result<Value, LabeledError> {
        Ok(Value::string(engine.get_help()?, call.head))
    }
}

// ── web search ──────────────────────────────────────────────────────────────

pub struct WebSearch;

impl SimplePluginCommand for WebSearch {
    type Plugin = WebPlugin;

    fn name(&self) -> &str {
        "web search"
    }

    fn description(&self) -> &str {
        "Search the web: table<url, title, publish_date, excerpts>."
    }

    fn extra_description(&self) -> &str {
        "Give a goal-style objective (what you want to learn and why) plus 1-3 \
         short keyword queries (3-6 words each); do not repeat the objective as a \
         query. With no queries the objective is used as the one query. Excerpts \
         are markdown chosen for the objective, so reading them is often enough; \
         use `web extract` for more of a page. Default mode is fast ($1/1k); \
         --mode basic|advanced is better on hard queries ($5/1k)."
    }

    fn signature(&self) -> Signature {
        Signature::build(self.name())
            .input_output_type(Type::Nothing, Type::table())
            .required(
                "objective",
                SyntaxShape::String,
                "What you want to find out, in a sentence.",
            )
            .rest(
                "queries",
                SyntaxShape::String,
                "Short keyword queries, 3-6 words each.",
            )
            .named(
                "mode",
                SyntaxShape::String,
                "turbo, fast (default), basic or advanced.",
                None,
            )
            .named(
                "max-results",
                SyntaxShape::Int,
                "Results to return, 1-10 (default 5).",
                Some('n'),
            )
            .named(
                "max-chars",
                SyntaxShape::Int,
                "Total excerpt characters across results (default 20000, max 100000).",
                None,
            )
            .named(
                "include",
                SyntaxShape::List(Box::new(SyntaxShape::String)),
                "Only these domains (e.g. [govt.nz .edu]).",
                None,
            )
            .named(
                "exclude",
                SyntaxShape::List(Box::new(SyntaxShape::String)),
                "Never these domains.",
                None,
            )
            .named(
                "after",
                SyntaxShape::String,
                "Only pages published on or after this date (YYYY-MM-DD).",
                None,
            )
            .switch(
                "raw",
                "Return the whole API reply (ids, usage, warnings).",
                None,
            )
            .category(category())
    }

    fn examples(&self) -> Vec<Example<'_>> {
        vec![
            Example {
                example: "web search 'How is kauri dieback spread and controlled in New Zealand?' 'kauri dieback spread' 'Phytophthora agathidicida control'",
                description: "An objective with two keyword queries",
                result: None,
            },
            Example {
                example: "web search 'nushell 0.117 release notes' --include [nushell.sh github.com] -n 3",
                description: "Restrict to two sites, three results",
                result: None,
            },
        ]
    }

    fn run(
        &self,
        _plugin: &WebPlugin,
        engine: &EngineInterface,
        call: &EvaluatedCall,
        _input: &Value,
    ) -> Result<Value, LabeledError> {
        let span = call.head;
        let mode: String = call
            .get_flag("mode")?
            .unwrap_or_else(|| api::DEFAULT_MODE.into());
        if !api::MODES.contains(&mode.as_str()) {
            return Err(api::err(
                format!("--mode must be one of {}", api::MODES.join(", ")),
                span,
            ));
        }
        let max_results = call
            .get_flag("max-results")?
            .unwrap_or(api::DEFAULT_RESULTS);
        let search = api::Search {
            objective: call.req(0)?,
            queries: call.rest(1)?,
            mode,
            max_results: api::bounded("max-results", max_results, api::MAX_RESULTS, span)?,
            max_chars: max_chars(call)?,
            include: strings(call, "include")?,
            exclude: strings(call, "exclude")?,
            after: call.get_flag("after")?,
        };
        let reply = api::search(&key(engine, call)?, &api::search_body(&search), span)?;
        Ok(if call.has_flag("raw")? {
            api::from_json(&reply, span)
        } else {
            api::search_table(&reply, span)
        })
    }
}

// ── web extract ─────────────────────────────────────────────────────────────

pub struct WebExtract;

impl SimplePluginCommand for WebExtract {
    type Plugin = WebPlugin;

    fn name(&self) -> &str {
        "web extract"
    }

    fn description(&self) -> &str {
        "Read web pages: table<url, title, publish_date, excerpts, full_content?, error?>."
    }

    fn extra_description(&self) -> &str {
        "URLs come as arguments or piped in (a string, a list, or a table with a \
         url column, such as `web search` output), at most 20. With --objective the \
         excerpts are the parts relevant to it; --full adds each page's whole text \
         as markdown, sharing the --max-chars budget. Pages that could not be \
         fetched come back as rows with an error column."
    }

    fn signature(&self) -> Signature {
        Signature::build(self.name())
            .input_output_types(vec![
                (Type::Nothing, Type::table()),
                (Type::String, Type::table()),
                (Type::List(Box::new(Type::Any)), Type::table()),
            ])
            .rest("urls", SyntaxShape::String, "Page URLs (http or https).")
            .named(
                "objective",
                SyntaxShape::String,
                "What to look for on the pages; focuses the excerpts.",
                None,
            )
            .switch("full", "Include each page's full text as markdown.", None)
            .named(
                "max-chars",
                SyntaxShape::Int,
                "Total characters across pages (default 20000, max 100000).",
                None,
            )
            .switch(
                "raw",
                "Return the whole API reply (ids, usage, warnings).",
                None,
            )
            .category(category())
    }

    fn examples(&self) -> Vec<Example<'_>> {
        vec![
            Example {
                example: "web extract https://www.nushell.sh/blog/ --objective 'latest release version and date'",
                description: "Excerpts of one page relevant to a question",
                result: None,
            },
            Example {
                example: "web search 'kauri dieback hygiene rules' -n 2 | web extract --full",
                description: "Read the top two search hits in full",
                result: None,
            },
        ]
    }

    fn run(
        &self,
        _plugin: &WebPlugin,
        engine: &EngineInterface,
        call: &EvaluatedCall,
        input: &Value,
    ) -> Result<Value, LabeledError> {
        let span = call.head;
        let mut urls: Vec<String> = call.rest(0)?;
        urls.extend(piped_urls(input)?);
        if urls.is_empty() {
            return Err(api::err("no URLs to read", span)
                .with_help("pass URLs as arguments or pipe them in"));
        }
        if urls.len() > api::MAX_URLS {
            return Err(api::err(
                format!("{} URLs, over the limit of {}", urls.len(), api::MAX_URLS),
                span,
            ));
        }
        if let Some(bad) = urls
            .iter()
            .find(|u| !(u.starts_with("https://") || u.starts_with("http://")))
        {
            return Err(api::err(format!("not an http(s) URL: {bad}"), span));
        }
        let extract = api::Extract {
            urls,
            objective: call.get_flag("objective")?,
            full: call.has_flag("full")?,
            max_chars: max_chars(call)?,
        };
        let reply = api::extract(&key(engine, call)?, &api::extract_body(&extract), span)?;
        Ok(if call.has_flag("raw")? {
            api::from_json(&reply, span)
        } else {
            api::extract_table(&reply, extract.full, span)
        })
    }
}

/// URLs from the pipeline: a string, a list of strings, or records (table
/// rows) with a `url` field.
fn piped_urls(input: &Value) -> Result<Vec<String>, LabeledError> {
    let one = |v: &Value| -> Result<String, LabeledError> {
        match v {
            Value::String { val, .. } => Ok(val.clone()),
            Value::Record { val, .. } => match val.get("url") {
                Some(Value::String { val, .. }) => Ok(val.clone()),
                _ => Err(api::err("piped record has no url string", v.span())),
            },
            other => Err(api::err(
                format!("cannot read a {} as a URL", other.get_type()),
                other.span(),
            )),
        }
    };
    match input {
        Value::Nothing { .. } => Ok(vec![]),
        Value::List { vals, .. } => vals.iter().map(one).collect(),
        v => Ok(vec![one(v)?]),
    }
}
