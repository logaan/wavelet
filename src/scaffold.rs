//! `wavelet new` — scaffold a fresh Wavelet project.
//!
//! A project is just a directory of `.wvl` files plus the small amount of glue
//! (a `.gitignore`, build/run scripts, a README) that makes it pleasant to work
//! on. Two templates exist: `cli` (the default) lays down a `wasi:cli/command`
//! program plus the domain model it imports; `http` lays down a web app whose
//! front end implements the `wasi:http/incoming-handler` interface. Either way
//! the entry point calls a separate domain-model component across the boundary.

use std::fs;
use std::path::{Path, PathBuf};

/// Which template to lay down. `Cli` is the default; `--type` selects another.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProjectKind {
    #[default]
    Cli,
    Http,
}

impl ProjectKind {
    /// Parse the `--type` value. The empty/absent case is handled by the caller,
    /// which defaults to [`ProjectKind::Cli`].
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "cli" => Ok(ProjectKind::Cli),
            "http" => Ok(ProjectKind::Http),
            other => Err(format!("unknown project type `{other}` (supported: cli, http)")),
        }
    }
}

/// Scaffold project `name` (a directory of that name in the current directory).
///
/// Returns the project root and every file written, so the CLI can report what
/// it created. Fails rather than overwrite if the directory already exists.
pub fn create(name: &str, kind: ProjectKind) -> Result<(PathBuf, Vec<PathBuf>), String> {
    let root = PathBuf::from(name);
    if root.exists() {
        return Err(format!("`{name}` already exists"));
    }
    // The directory name is whatever the user typed; the WIT package namespace
    // has to be a valid label, so derive a slug from it.
    let slug = slugify(name)?;

    let mut written = Vec::new();
    let mut write = |rel: &str, contents: String, exec: bool| -> Result<(), String> {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("creating {}: {e}", parent.display()))?;
        }
        fs::write(&path, contents).map_err(|e| format!("writing {}: {e}", path.display()))?;
        if exec {
            set_executable(&path)?;
        }
        written.push(path);
        Ok(())
    };

    // The .gitignore is the same for every template; the rest depends on kind.
    write(".gitignore", GITIGNORE.to_string(), false)?;
    match kind {
        ProjectKind::Cli => {
            write("README.md", cli_readme(name), false)?;
            write("src/greeting.wvl", greeting_wvl(&slug), false)?;
            write("src/main.wvl", main_wvl(&slug), false)?;
            write("scripts/build.sh", cli_build_sh(&slug), true)?;
            write("scripts/run.sh", RUN_SH.to_string(), true)?;
        }
        ProjectKind::Http => {
            write("README.md", http_readme(name), false)?;
            write("src/greeting.wvl", greeting_wvl(&slug), false)?;
            write("src/app.wvl", app_wvl(&slug), false)?;
            write("scripts/build.sh", http_build_sh(&slug), true)?;
            write("scripts/serve.sh", SERVE_SH.to_string(), true)?;
        }
    }

    Ok((root, written))
}

/// Turn an arbitrary project name into a WIT label usable as a package
/// namespace: lowercase, words of letters/digits separated by single hyphens,
/// starting with a letter.
fn slugify(name: &str) -> Result<String, String> {
    // Only the final path component names the package (e.g. `tmp/my-app` -> `my-app`).
    let base = Path::new(name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(name);

    let mut out = String::new();
    let mut prev_dash = false;
    for ch in base.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    let slug = out.trim_matches('-').to_string();

    // WIT labels must start with a letter; prefix one if the name was all digits
    // or punctuation.
    let slug = match slug.chars().next() {
        Some(c) if c.is_ascii_alphabetic() => slug,
        Some(_) => format!("app-{slug}"),
        None => return Err(format!("`{name}` has no letters or digits to name a package after")),
    };
    Ok(slug)
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)
        .map_err(|e| format!("stat {}: {e}", path.display()))?
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).map_err(|e| format!("chmod {}: {e}", path.display()))
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

const GITIGNORE: &str = "# Build artifacts (scripts/build.sh writes here).\n/out\n";

const SERVE_SH: &str = "\
#!/usr/bin/env bash
# Build the project, then serve it locally with `wasmtime serve`.
# Open http://localhost:8080 and click \"+1\".
set -euo pipefail
here=\"$(cd \"$(dirname \"$0\")/..\" && pwd)\"
cd \"$here\"

scripts/build.sh
exec wasmtime serve out/app.wasm
";

// ---------------------------------------------------------------------------
// cli template
// ---------------------------------------------------------------------------

const RUN_SH: &str = "\
#!/usr/bin/env bash
# Build the project, then run it with `wasmtime`. Any extra arguments are passed
# through to the program (try `scripts/run.sh Ada`).
set -euo pipefail
here=\"$(cd \"$(dirname \"$0\")/..\" && pwd)\"
cd \"$here\"

scripts/build.sh
exec wasmtime run out/app.wasm \"$@\"
";

fn cli_build_sh(_slug: &str) -> String {
    "\
#!/usr/bin/env bash
# Compile every component in src/ into the (git-ignored) out/ directory and link
# them into a single runnable component, out/app.wasm. `wavelet build` composes
# the project's components (wiring each cross-component import to the component
# that exports it) into one artifact via a generated out/app.wac + `wac`; the
# individual out/<pkg>.wasm components are left alongside it.
set -euo pipefail
here=\"$(cd \"$(dirname \"$0\")/..\" && pwd)\"
cd \"$here\"

wavelet build src/*.wvl -o out
echo \"built out/app.wasm\"
"
    .to_string()
}

fn cli_readme(name: &str) -> String {
    format!(
        "\
# {name}

A [Wavelet](https://logaan.github.io/wavelet/) command-line component: it prints
a greeting for the name you give it.

## Run it

```sh
scripts/run.sh           # greets the world
scripts/run.sh Ada       # greets Ada
```

`scripts/build.sh` compiles `src/*.wvl` into the git-ignored `out/` directory
and links them into `out/app.wasm`; `scripts/run.sh` does that and then runs the
component with [`wasmtime`](https://docs.wasmtime.dev/).

## Layout

- `src/main.wvl` — the entry point. Implements the `wasi:cli/run` interface and
  exports `run`, reading its arguments from `wasi:cli/environment` and writing
  to `wasi:cli/stdout`.
- `src/greeting.wvl` — the domain model. The pure `greet` function, imported by
  `main.wvl` across the component boundary.

## Learn more

Wavelet is a homoiconic language for the WebAssembly Component Model. See the
documentation at <https://logaan.github.io/wavelet/>.
"
    )
}

fn greeting_wvl(slug: &str) -> String {
    format!(
        "\
// greeting.wvl — the domain model.
//
// Pure logic with no knowledge of the command line: just how to phrase a
// greeting. Keeping it in its own component means main.wvl is only plumbing, and
// the same logic could back an HTTP front end, a test, or a library unchanged.
Package \"{slug}:greeting@0.1.0\"

/// A friendly greeting for `name`.
Export greet
Def greet Fn {{name: string}}
  str-cat[\"Hello, \" name \"!\"]
"
    )
}

fn main_wvl(slug: &str) -> String {
    format!(
        "\
// main.wvl — the command-line entry point.
//
// This component implements the `wasi:cli/run` interface: a CLI host (here,
// `wasmtime`) calls `run` on startup. The interface is exported by name via
// `Export {{iface: …}}` — `run: func() -> result` — with no compiler-special-cased
// `wasi:cli/command` target.
//
// Output and arguments are ordinary calls into imported WASI interfaces, lowered
// through the generic WIT bridge: `wasi:cli/stdout` for the output stream,
// `wasi:io/streams` to write it, and `wasi:cli/environment` for the argument
// vector. Their WIT is fetched into `wit/` by `wkg`.
Package \"{slug}:main@0.1.0\"

Import {{pkg: \"{slug}:greeting/api\" as: greeting}}
Import {{pkg: \"wasi:cli/stdout@0.2.0\" as: stdout}}
Import {{pkg: \"wasi:cli/environment@0.2.0\" as: env}}
Import {{pkg: \"wasi:io/streams@0.2.0\" as: streams}}

// The first *user* argument, or \"world\" when none was given. `get-arguments`
// includes the program name as `argv[0]`, so the user's first word is `argv[1]`.
Def who Fn {{}}
  Let {{a: env/get-arguments()}}
    If gt[len(a) 1] head(tail(a)) \"world\"

// Write a line to stdout, then drop the stream (a child resource that must be
// released). A Wavelet string lowers to the `list<u8>` the stream expects.
Def say Fn {{line: string}}
  Let {{out: stdout/get-stdout()}}
    Do [streams/blocking-write-and-flush[out line]
        streams/drop-output-stream(out)]

Export {{iface: \"wasi:cli/run\" name: run result: result}}
Def run Fn {{}}
  Do [say(str-cat[greeting/greet(who()) \"\\n\"])
      ok(0)]
"
    )
}

// ---------------------------------------------------------------------------
// http template
// ---------------------------------------------------------------------------

fn http_build_sh(_slug: &str) -> String {
    "\
#!/usr/bin/env bash
# Compile every component in src/ into the (git-ignored) out/ directory and link
# them into a single deployable component, out/app.wasm. `wavelet build` composes
# the project's components (wiring each cross-component import to the component
# that exports it) into one artifact via a generated out/app.wac + `wac`; the
# individual out/<pkg>.wasm components are left alongside it.
set -euo pipefail
here=\"$(cd \"$(dirname \"$0\")/..\" && pwd)\"
cd \"$here\"

wavelet build src/*.wvl -o out
echo \"built out/app.wasm\"
"
    .to_string()
}

fn http_readme(name: &str) -> String {
    format!(
        "\
# {name}

A [Wavelet](https://logaan.github.io/wavelet/) HTTP component: a web page that
greets you and echoes the path you requested.

## Run it

```sh
scripts/serve.sh
```

Then open <http://localhost:8080> (try a path like `/hello`). Each request lands
in `handle`, which builds the page — the greeting wording comes from the
`greet` function in `src/greeting.wvl`, across the component boundary.

`scripts/build.sh` compiles `src/*.wvl` into the git-ignored `out/` directory
and links them into `out/app.wasm`; `scripts/serve.sh` does that and then runs
the component with [`wasmtime serve`](https://docs.wasmtime.dev/).

## Layout

- `src/app.wvl` — the web front end. Implements the `wasi:http/incoming-handler`
  interface: every request lands in `handle`, which renders the page.
- `src/greeting.wvl` — the domain model. The pure `greet` function, with no
  knowledge of HTTP, imported by `app.wvl` across the component boundary.

## Learn more

Wavelet is a homoiconic language for the WebAssembly Component Model. See the
documentation at <https://logaan.github.io/wavelet/>.
"
    )
}

fn app_wvl(slug: &str) -> String {
    format!(
        "\
// app.wvl — the HTTP front end.
//
// This component implements the `wasi:http/incoming-handler` interface: an HTTP
// host (here, `wasmtime serve`) calls `handle` for every request. The interface
// is exported by name via `Export {{iface: …}}`, and the response pipeline is
// driven by ordinary calls into the imported `wasi:http/types` and
// `wasi:io/streams` interfaces — no compiler-special-cased `http/*` magic. The
// WIT for those interfaces is fetched into `wit/` by `wkg`.
//
// The page is stateless: it greets the world (wording from the greeting
// component, across the boundary) and echoes the path you requested.
Package \"{slug}:app@0.1.0\"

Import {{pkg: \"wasi:http/types@0.2.0\" as: http}}
Import {{pkg: \"wasi:io/streams@0.2.0\" as: streams}}
Import {{pkg: \"{slug}:greeting/api\" as: greeting}}

// The path-and-query of a request, defaulting to \"/\". Several wasi:http
// resources share a `path-with-query` op, so the resource-qualified name picks
// the one on `incoming-request`.
Def path-of Fn {{request: incoming-request}}
  Match http/incoming-request-path-with-query(request)
    [(some(p)  p)
     (none     \"/\")]

// The HTML page: a greeting from the domain component, plus the request path.
Def page Fn {{path: string}}
  str-cat[
    \"<!doctype html>\\n\"
    \"<title>{slug}</title>\\n\"
    \"<h1>\" greeting/greet(\"world\") \"</h1>\\n\"
    \"<p>You requested: \" path \"</p>\\n\"
  ]

// Write the page bytes into a body's stream, then drop the stream (a child
// resource that must be released before the body is finished). Resource ops are
// reached by their bare name when unique, or resource-qualified when not.
Def write-page Fn {{body: outgoing-body, html: string}}
  Match http/outgoing-body-write(body)
    [(ok(stream)
       Do [streams/blocking-write-and-flush[stream html]
           streams/drop-output-stream(stream)])
     (err(e)  0)]

// wasi:http/incoming-handler: build the response, hand it to the outparam, write
// the page, then finish the body. Each step is a plain call into the imported
// wasi:http / wasi:io interfaces, lowered through the generic WIT bridge.
Export {{iface: \"wasi:http/incoming-handler\" name: handle
         params: {{request: incoming-request response-out: response-outparam}}}}
Def handle Fn {{request: incoming-request, response-out: response-outparam}}
  Let {{response: http/outgoing-response(http/fields())}}
    Match http/outgoing-response-body(response)
      [(ok(body)
         Do [http/response-outparam-set[response-out ok(response)]
             write-page[body page(path-of(request))]
             http/outgoing-body-finish[body none]])
       (err(e)  0)]
"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_normalizes_names() {
        assert_eq!(slugify("my-app").unwrap(), "my-app");
        assert_eq!(slugify("My App").unwrap(), "my-app");
        assert_eq!(slugify("Cool_Thing!!").unwrap(), "cool-thing");
        assert_eq!(slugify("tmp/nested/proj").unwrap(), "proj");
        assert_eq!(slugify("123go").unwrap(), "app-123go");
        assert!(slugify("!!!").is_err());
    }

    #[test]
    fn cli_is_the_default_kind() {
        assert_eq!(ProjectKind::default(), ProjectKind::Cli);
        assert_eq!(ProjectKind::parse("cli").unwrap(), ProjectKind::Cli);
        assert_eq!(ProjectKind::parse("http").unwrap(), ProjectKind::Http);
        assert!(ProjectKind::parse("grpc").is_err());
    }

    /// A fresh temp directory unique to this test, cleaned on entry and exit.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wavelet-scaffold-{}-{tag}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn create_lays_down_a_cli_project() {
        let dir = scratch("cli");
        let name = dir.join("widgets");
        let name_str = name.to_str().unwrap();

        let (root, files) = create(name_str, ProjectKind::Cli).unwrap();
        assert_eq!(root, name);

        for rel in [
            ".gitignore",
            "README.md",
            "src/main.wvl",
            "src/greeting.wvl",
            "scripts/build.sh",
            "scripts/run.sh",
        ] {
            assert!(root.join(rel).is_file(), "missing {rel}");
        }
        assert_eq!(files.len(), 6);

        // The slug derived from the directory name lands in the package ids.
        let main = fs::read_to_string(root.join("src/main.wvl")).unwrap();
        assert!(main.contains("Package \"widgets:main@0.1.0\""), "{main}");
        // The cli entry exports wasi:cli/run generically and drops `Target`.
        assert!(main.contains("wasi:cli/run"), "{main}");
        assert!(!main.contains("Target"), "cli template should not use Target: {main}");
        assert!(main.contains("widgets:greeting/api"), "{main}");
        let greeting = fs::read_to_string(root.join("src/greeting.wvl")).unwrap();
        assert!(greeting.contains("Package \"widgets:greeting@0.1.0\""), "{greeting}");

        // The run script is executable.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(root.join("scripts/run.sh"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "run.sh not executable");
        }

        // Refuses to clobber an existing directory.
        assert!(create(name_str, ProjectKind::Cli).is_err());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn create_lays_down_an_http_project() {
        let dir = scratch("http");
        let name = dir.join("widgets");
        let name_str = name.to_str().unwrap();

        let (root, files) = create(name_str, ProjectKind::Http).unwrap();
        assert_eq!(root, name);

        for rel in [
            ".gitignore",
            "README.md",
            "src/app.wvl",
            "src/greeting.wvl",
            "scripts/build.sh",
            "scripts/serve.sh",
        ] {
            assert!(root.join(rel).is_file(), "missing {rel}");
        }
        assert_eq!(files.len(), 6);

        let app = fs::read_to_string(root.join("src/app.wvl")).unwrap();
        assert!(app.contains("Package \"widgets:app@0.1.0\""), "{app}");
        // The http front end exports the handler interface generically and
        // imports wasi:http/types + wasi:io/streams directly — no `Target`.
        assert!(app.contains("wasi:http/incoming-handler"), "{app}");
        assert!(app.contains("wasi:http/types"), "{app}");
        assert!(app.contains("wasi:io/streams"), "{app}");
        assert!(!app.contains("Target"), "http template should not use Target: {app}");
        assert!(app.contains("widgets:greeting/api"), "{app}");
        let greeting = fs::read_to_string(root.join("src/greeting.wvl")).unwrap();
        assert!(greeting.contains("Package \"widgets:greeting@0.1.0\""), "{greeting}");

        let _ = fs::remove_dir_all(&dir);
    }
}
