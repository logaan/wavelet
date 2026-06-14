//! `wavelet new` — scaffold a fresh Wavelet project.
//!
//! A project is just a directory of `.wvl` files plus the small amount of glue
//! (a `.gitignore`, build/serve scripts, a README) that makes it pleasant to
//! work on. The `http` template lays down a two-component web app: a front end
//! that implements the `wasi:http/incoming-handler` interface and a domain
//! model it calls across the component boundary.

use std::fs;
use std::path::{Path, PathBuf};

/// Which template to lay down. `Http` is the only kind today — and the default —
/// but the enum (and the `--type` flag that selects it) exists so more templates
/// can join without changing the CLI surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectKind {
    Http,
}

impl ProjectKind {
    /// Parse the `--type` value. The empty/absent case is handled by the caller,
    /// which defaults to `Http`.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "http" => Ok(ProjectKind::Http),
            other => Err(format!("unknown project type `{other}` (supported: http)")),
        }
    }
}

/// Scaffold project `name` (a directory of that name in the current directory).
///
/// Returns the project root and every file written, so the CLI can report what
/// it created. Fails rather than overwrite if the directory already exists.
pub fn create(name: &str, kind: ProjectKind) -> Result<(PathBuf, Vec<PathBuf>), String> {
    let ProjectKind::Http = kind; // exhaustive today; keeps the match honest as kinds grow

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

    write(".gitignore", GITIGNORE.to_string(), false)?;
    write("README.md", readme(name), false)?;
    write("src/counter.wvl", counter_wvl(&slug), false)?;
    write("src/app.wvl", app_wvl(&slug), false)?;
    write("scripts/build.sh", build_sh(&slug), true)?;
    write("scripts/serve.sh", SERVE_SH.to_string(), true)?;

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

fn build_sh(slug: &str) -> String {
    format!(
        "\
#!/usr/bin/env bash
# Compile every component in src/ into the (git-ignored) out/ directory, then
# link them into a single deployable component, out/app.wasm.
set -euo pipefail
here=\"$(cd \"$(dirname \"$0\")/..\" && pwd)\"
cd \"$here\"

wavelet build src/*.wvl -o out
wavelet compose out/{slug}-app.wasm out/{slug}-counter.wasm -o out/app.wasm
echo \"built out/app.wasm\"
"
    )
}

fn readme(name: &str) -> String {
    format!(
        "\
# {name}

A [Wavelet](https://logaan.github.io/wavelet/) HTTP component: a web page that
shows a number, with a button that increments it.

## Run it

```sh
scripts/serve.sh
```

Then open <http://localhost:8080> and click **+1**. Each click reloads the page
with the next count, computed by the `increment` function in
`src/counter.wvl`.

`scripts/build.sh` compiles `src/*.wvl` into the git-ignored `out/` directory
and links them into `out/app.wasm`; `scripts/serve.sh` does that and then runs
the component with [`wasmtime serve`](https://docs.wasmtime.dev/).

## Layout

- `src/app.wvl` — the web front end. Implements the `wasi:http/incoming-handler`
  interface: every request lands in `handle`, which renders the page.
- `src/counter.wvl` — the domain model. Pure counter logic, with no knowledge of
  HTTP, imported by `app.wvl` across the component boundary.

## Learn more

Wavelet is a homoiconic language for the WebAssembly Component Model. See the
documentation at <https://logaan.github.io/wavelet/>.
"
    )
}

fn counter_wvl(slug: &str) -> String {
    format!(
        "\
// counter.wvl — the domain model.
//
// Pure counter logic with no knowledge of HTTP. Keeping the rules in their own
// component means the web front end (app.wvl) is just plumbing, and the same
// logic could back a CLI, a test, or a different front end unchanged.
Package \"{slug}:counter@0.1.0\"

/// One more than `n`.
Export increment
Def increment Fn {{n: s64}}
  add[n 1]
"
    )
}

fn app_wvl(slug: &str) -> String {
    format!(
        "\
// app.wvl — the HTTP front end.
//
// This component implements the `wasi:http/incoming-handler` interface: an HTTP
// host (here, `wasmtime serve`) calls `handle` for every request. Wavelet needs
// no special \"http\" support — adopting the `wasi:http/proxy` world and exporting
// the interface is the whole story, exactly like implementing any other WIT
// interface.
//
// State lives in the URL: the \"+1\" link carries the next count, which the
// domain model's `increment` computes. So the server itself stays stateless.
Package \"{slug}:app@0.1.0\"
Target \"wasi:http/proxy\"

Import {{pkg: \"wasi:http/types@0.2.0\" as: http}}
Import {{pkg: \"{slug}:counter/api\" as: counter}}

// The trailing number of a \"...=N\" query string, defaulting to 0.
Def count-in Fn {{query: string}}
  Match read(head(reverse(split[query \"=\"])))
    [(ok(n)   to-s64(n))
     (err(e)  0)]

// The path-and-query of a request, defaulting to \"/\".
Def path-of Fn {{request: incoming-request}}
  Match http/path-with-query(request)
    [(some(p)  p)
     (none     \"/\")]

// The HTML page for a given count. The button links back with the next count.
Def page Fn {{count: s64}}
  str-cat[
    \"<!doctype html>\\n\"
    \"<title>{slug}</title>\\n\"
    \"<h1>\" to-string(count) \"</h1>\\n\"
    \"<a href=\\\"/?count=\" to-string(counter/increment(count)) \"\\\">+1</a>\\n\"
  ]

// wasi:http/incoming-handler: render the page and write it back as the response.
Export {{iface: \"wasi:http/incoming-handler\" name: handle
         params: {{request: incoming-request response-out: response-outparam}}}}
Def handle Fn {{request: incoming-request, response-out: response-outparam}}
  Let {{count:    count-in(path-of(request))
        response: http/outgoing-response(http/fields())
        body:     http/body(response)}}
    // The standard wasi:http 0.2 response sequence: hand the response to the
    // outparam, stream the HTML body, then finish it.
    Do [
      http/set[response-out ok(response)]
      http/blocking-write-and-flush[http/write(body) page(count)]
      http/finish[body none]
    ]
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
    fn create_lays_down_the_project() {
        let dir = std::env::temp_dir().join(format!("wavelet-scaffold-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let name = dir.join("widgets");
        let name_str = name.to_str().unwrap();

        let (root, files) = create(name_str, ProjectKind::Http).unwrap();
        assert_eq!(root, name);

        for rel in [
            ".gitignore",
            "README.md",
            "src/app.wvl",
            "src/counter.wvl",
            "scripts/build.sh",
            "scripts/serve.sh",
        ] {
            assert!(root.join(rel).is_file(), "missing {rel}");
        }
        assert_eq!(files.len(), 6);

        // The slug derived from the directory name lands in the package ids.
        let app = fs::read_to_string(root.join("src/app.wvl")).unwrap();
        assert!(app.contains("Package \"widgets:app@0.1.0\""), "{app}");
        assert!(app.contains("widgets:counter/api"), "{app}");
        let counter = fs::read_to_string(root.join("src/counter.wvl")).unwrap();
        assert!(counter.contains("Package \"widgets:counter@0.1.0\""), "{counter}");

        // Scripts are executable.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(root.join("scripts/serve.sh"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "serve.sh not executable");
        }

        // Refuses to clobber an existing directory.
        assert!(create(name_str, ProjectKind::Http).is_err());

        let _ = fs::remove_dir_all(&dir);
    }
}
