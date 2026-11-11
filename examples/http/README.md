# examples/http

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

## Verify locally

After `scripts/build.sh`, run `python3 scripts/smoke.py`. This starts a temporary
server and checks its status, HTML content type, greeting, and escaped query.
The WASI 0.2.0 packages in `wit/deps` are vendored with versions and digests in
`wkg.lock`; an unavailable registry refresh does not prevent local builds.
