use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProjectKind {
    #[default]
    Cli,
    Http,
}

impl ProjectKind {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "cli" => Ok(ProjectKind::Cli),
            "http" => Ok(ProjectKind::Http),
            other => Err(format!(
                "unknown project type `{other}` (supported: cli, http)"
            )),
        }
    }
}

pub fn create(name: &str, kind: ProjectKind) -> Result<(PathBuf, Vec<PathBuf>), String> {
    let root = PathBuf::from(name);
    if root.exists() {
        return Err(format!("`{name}` already exists"));
    }

    let slug = slugify(name)?;

    let mut written = Vec::new();
    let mut write = |rel: &str, contents: String, exec: bool| -> Result<(), String> {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("creating {}: {e}", parent.display()))?;
        }
        fs::write(&path, contents).map_err(|e| format!("writing {}: {e}", path.display()))?;
        if exec {
            set_executable(&path)?;
        }
        written.push(path);
        Ok(())
    };

    write(".gitignore", GITIGNORE.to_string(), false)?;
    match kind {
        ProjectKind::Cli => {
            write("README.md", cli_readme(name), false)?;
            write("src/greeting.wlt", greeting_wlt(&slug), false)?;
            write("src/main.wlt", main_wlt(&slug), false)?;
            write("scripts/build.sh", cli_build_sh(&slug), true)?;
            write("scripts/run.sh", RUN_SH.to_string(), true)?;
        }
        ProjectKind::Http => {
            write("README.md", http_readme(name), false)?;
            write("src/greeting.wlt", greeting_wlt(&slug), false)?;
            write("src/app.wlt", app_wlt(&slug), false)?;
            write("scripts/build.sh", http_build_sh(&slug), true)?;
            write("scripts/serve.sh", SERVE_SH.to_string(), true)?;
        }
    }

    Ok((root, written))
}

fn slugify(name: &str) -> Result<String, String> {
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

    let slug = match slug.chars().next() {
        Some(c) if c.is_ascii_alphabetic() => slug,
        Some(_) => format!("app-{slug}"),
        None => {
            return Err(format!(
                "`{name}` has no letters or digits to name a package after"
            ));
        }
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
set -euo pipefail
here=\"$(cd \"$(dirname \"$0\")/..\" && pwd)\"
cd \"$here\"

scripts/build.sh
exec wasmtime serve \"$@\" out/app.wasm
";

const RUN_SH: &str = "\
#!/usr/bin/env bash
set -euo pipefail
here=\"$(cd \"$(dirname \"$0\")/..\" && pwd)\"
cd \"$here\"

scripts/build.sh
exec wasmtime run out/app.wasm \"$@\"
";

fn cli_build_sh(_slug: &str) -> String {
    "\
#!/usr/bin/env bash
set -euo pipefail
here=\"$(cd \"$(dirname \"$0\")/..\" && pwd)\"
cd \"$here\"

wavelet build src/*.wlt -o out
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

`scripts/build.sh` compiles `src/*.wlt` into the git-ignored `out/` directory
and links them into `out/app.wasm`; `scripts/run.sh` does that and then runs the
component with [`wasmtime`](https://docs.wasmtime.dev/).

## Layout

- `src/main.wlt` — the entry point. Implements the `wasi:cli/run` interface and
  exports `run`, reading its arguments from `wasi:cli/environment` and writing
  to `wasi:cli/stdout`.
- `src/greeting.wlt` — the domain model. The pure `greet` function, imported by
  `main.wlt` across the component boundary.

## Learn more

Wavelet is a homoiconic language for the WebAssembly Component Model. See the
documentation at <https://logaan.github.io/wavelet/>.
"
    )
}

fn greeting_wlt(slug: &str) -> String {
    format!(
        "\
Package \"{slug}:greeting@0.1.0\"

Export greet
Def greet Fn {{name: string}}
  str-cat(\"Hello, \" name \"!\")
"
    )
}

fn main_wlt(slug: &str) -> String {
    format!(
        "\
Package \"{slug}:main@0.1.0\"

Import {{pkg: \"{slug}:greeting/api\" as: greeting}}
Import {{pkg: \"wasi:cli/stdout@0.2.0\" as: stdout}}
Import {{pkg: \"wasi:cli/environment@0.2.0\" as: env}}
Import {{pkg: \"wasi:io/streams@0.2.0\" as: streams}}

Def who Fn {{}}
  Let {{a: env/get-arguments()}}
    If gt(len(a) 1) head(tail(a)) \"world\"

Def say Fn {{line: string}}
  Let {{out: stdout/get-stdout()}}
    Do [streams/blocking-write-and-flush(out line)
        streams/drop-output-stream(out)]

Export {{iface: \"wasi:cli/run\" name: run result: result}}
Def run Fn {{}}
  Do [say(str-cat(greeting/greet(who()) \"\\n\"))
      ok(0)]
"
    )
}

fn http_build_sh(_slug: &str) -> String {
    "\
#!/usr/bin/env bash
set -euo pipefail
here=\"$(cd \"$(dirname \"$0\")/..\" && pwd)\"
cd \"$here\"

wavelet build src/*.wlt -o out
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
`greet` function in `src/greeting.wlt`, across the component boundary.

`scripts/build.sh` compiles `src/*.wlt` into the git-ignored `out/` directory
and links them into `out/app.wasm`; `scripts/serve.sh` does that and then runs
the component with [`wasmtime serve`](https://docs.wasmtime.dev/).

## Layout

- `src/app.wlt` — the web front end. Implements the `wasi:http/incoming-handler`
  interface: every request lands in `handle`, which renders the page.
- `src/greeting.wlt` — the domain model. The pure `greet` function, with no
  knowledge of HTTP, imported by `app.wlt` across the component boundary.

## Deployment

Deploy `out/app.wasm` to a host that supports the component's WASI 0.2 imports
and the `wasi:http/incoming-handler` export. Check the host's supported interface
versions and configure request routing, capabilities, and TLS for that environment.
Packaging and upload commands depend on the host. Validate the component and
run the local smoke test before deploying.

## Learn more

Wavelet is a homoiconic language for the WebAssembly Component Model. See the
documentation at <https://logaan.github.io/wavelet/>.
"
    )
}

fn app_wlt(slug: &str) -> String {
    format!(
        "\
Package \"{slug}:app@0.1.0\"

Import {{pkg: \"wasi:http/types@0.2.0\" as: http}}
Import {{pkg: \"wasi:io/streams@0.2.0\" as: streams}}
Import {{pkg: \"{slug}:greeting/api\" as: greeting}}

Def path-of Fn {{request: borrow(incoming-request)}}
  Match http/incoming-request-path-with-query(request)
    [(some(p)  p)
     (none     \"/\")]

Def html-escape Fn {{text: string}}
  Let {{amp: join(split(text \"&\") \"&amp;\")
       lt: join(split(amp \"<\") \"&lt;\")}}
    join(split(lt \">\") \"&gt;\")

Def page Fn {{path: string}}
  str-cat(
    \"<!doctype html>\\n\"
    \"<title>{slug}</title>\\n\"
    \"<h1>\" greeting/greet(\"world\") \"</h1>\\n\"
    \"<p>You requested: \" html-escape(path) \"</p>\\n\"
  )

Def write-page Fn {{body: borrow(outgoing-body), html: string}}
  Match http/outgoing-body-write(body)
    [(ok(stream)
       Do [streams/blocking-write-and-flush(stream html)
           streams/drop-output-stream(stream)])
     (err() drop(0))]

Export {{iface: \"wasi:http/incoming-handler\" name: handle
         params: {{request: incoming-request response-out: response-outparam}}}}
Def handle Fn {{request: incoming-request, response-out: response-outparam}}
  Let {{headers: http/fields()
        header-result: http/fields-append(headers \"content-type\" \"text/html; charset=utf-8\")
        response: http/outgoing-response(headers)
        path: path-of(request)
        dropped: http/drop-incoming-request(request)}}
    Match http/outgoing-response-body(response)
      [(ok(body)
         Do [http/response-outparam-set(response-out ok(response))
             write-page(body page(path))
             http/outgoing-body-finish(body none)
             drop(0)])
       (err() drop(0))]
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

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("wavelet-scaffold-{}-{tag}", std::process::id()));
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
            "src/main.wlt",
            "src/greeting.wlt",
            "scripts/build.sh",
            "scripts/run.sh",
        ] {
            assert!(root.join(rel).is_file(), "missing {rel}");
        }
        assert_eq!(files.len(), 6);

        let main = fs::read_to_string(root.join("src/main.wlt")).unwrap();
        assert!(main.contains("Package \"widgets:main@0.1.0\""), "{main}");

        assert!(main.contains("wasi:cli/run"), "{main}");
        assert!(
            !main.contains("Target"),
            "cli template should not use Target: {main}"
        );
        assert!(main.contains("widgets:greeting/api"), "{main}");
        let greeting = fs::read_to_string(root.join("src/greeting.wlt")).unwrap();
        assert!(
            greeting.contains("Package \"widgets:greeting@0.1.0\""),
            "{greeting}"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(root.join("scripts/run.sh"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "run.sh not executable");
        }

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
            "src/app.wlt",
            "src/greeting.wlt",
            "scripts/build.sh",
            "scripts/serve.sh",
        ] {
            assert!(root.join(rel).is_file(), "missing {rel}");
        }
        assert_eq!(files.len(), 6);

        let app = fs::read_to_string(root.join("src/app.wlt")).unwrap();
        assert!(app.contains("Package \"widgets:app@0.1.0\""), "{app}");

        assert!(app.contains("wasi:http/incoming-handler"), "{app}");
        assert!(app.contains("wasi:http/types"), "{app}");
        assert!(app.contains("wasi:io/streams"), "{app}");
        assert!(
            !app.contains("Target"),
            "http template should not use Target: {app}"
        );
        assert!(app.contains("widgets:greeting/api"), "{app}");
        let greeting = fs::read_to_string(root.join("src/greeting.wlt")).unwrap();
        assert!(
            greeting.contains("Package \"widgets:greeting@0.1.0\""),
            "{greeting}"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
