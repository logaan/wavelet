// wavelet-as-markup.typ — Documents are data: a Markdown-class markup, in user space.
// Render: `typst compile dev-notes/wavelet-as-markup.typ`

#set document(title: "Wavelet as a markup language", author: "Claude (Opus 4.8)")
#set page(paper: "a4", margin: (x: 2.0cm, y: 2.0cm), numbering: "1")
#set par(justify: true, leading: 0.62em)
#set text(size: 10pt, font: "New Computer Modern")
#show raw: set text(font: "DejaVu Sans Mono", size: 8.5pt)
#show raw.where(block: false): box.with(
  fill: luma(240), inset: (x: 3pt, y: 0pt), outset: (y: 3pt), radius: 2pt,
)
#show raw.where(block: true): block.with(
  fill: luma(246), inset: 7pt, radius: 4pt, width: 100%,
)
#show link: set text(fill: rgb("#1f6feb"))
#set heading(numbering: "1")
#show heading.where(level: 1): set text(size: 13pt)
#show heading.where(level: 2): set text(size: 11pt)

#let note(body) = block(
  fill: rgb("#eef3fb"), inset: 9pt, radius: 4pt, width: 100%,
  above: 0.8em, below: 0.8em, body,
)

// Two-column "Markdown vs. Wavelet" comparison. `md` and `wv` are raw blocks.
#let pair(title, md, wv) = block(width: 100%, above: 0.7em, below: 0.7em, [
  #text(weight: "bold")[#title]
  #v(1pt)
  #grid(
    columns: (1fr, 1fr), column-gutter: 9pt, row-gutter: 3pt,
    text(size: 7.5pt, fill: luma(45%))[CORE MARKDOWN],
    text(size: 7.5pt, fill: luma(45%))[WAVELET],
    md, wv,
  )
])

#block(fill: luma(96%), width: 100%, inset: 12pt, radius: 5pt, [
  #text(size: 17pt, weight: "bold")[Documents are data]
  #v(2pt)
  #text(size: 11pt)[A Markdown-class markup language as a Wavelet library — no
  changes to the core or the standard library]
  #v(4pt)
  #text(size: 9pt, fill: luma(35%))[
    Dev note · Wavelet · 2026-06-26 · Author: Claude (Opus 4.8)
  ]
])

Wavelet is homoiconic over WAVE: a source file *is* a tree of Component-Model
values, the same way a Lisp file is a tree of s-expressions (`design.md` §1).
Everything a markup language needs is already sitting in that substrate, unused:

- *Strings carry prose.* `"the quick brown fox"` is a leaf value, not a token
  stream to be re-lexed. Markdown's escaping problem (`\*`, `&amp;`, `<`) mostly
  evaporates — see §11.
- *Lists carry sequences.* `[a b c]` is the document's spine: a sequence of
  blocks, or a run of inline content.
- *Records carry attributes.* `{href: "…"}` is how a link, an image, or a fenced
  block names its parts.
- *TitleCase macros read like markup.* A `TitleCase` head is reader sugar for a
  macro call whose arguments are read *paren-free*, by declared arity, and which
  *nests without delimiters* (`design.md` §2.4). `Heading 1 "Hi"` is not a
  function call dressed up — it is the reader consuming exactly two following
  forms. This is the whole trick: it is what lets prose markup avoid the
  parenthesis soup that sinks every "XML-as-Lisp" attempt.
- *Macros are components.* A markup library publishes a macro manifest; importing
  it with `macros: true` registers those TitleCase arities with the reader
  (`design.md` §6.3). Nothing in `wavelet:std/core` changes; the language does
  not know markup exists.

So the claim of this note: a markup with the full expressive range of *core
Markdown* (Gruber's original syntax) is a *user library* — a handful of
`DefMacro`s that expand to plain data constructors, plus one rendering function.
The document file is the document. It compiles to a component whose value is its
own content tree, and "render to HTML" is just composition with a renderer
(which may be written in Rust). The rest of this note is mostly examples.

#note[
  *Conventions in the examples.* Every Wavelet file below opens with
  `Import {pkg: "prose:markup/dsl" macros: true open: true}`. `macros: true`
  teaches the reader the TitleCase arities; `open: true` splats the library's
  names (`Doc`, `Heading`, …, and the function `render-html`) in unqualified.
  That one line is the only ceremony. We elide it after §1.
]

= The whole idea in one file

Here is a complete document. The file *is* the post; it is also a `wasi:cli`
component that prints itself as HTML.

```
// hello.wvl — this file is the document
Package "blog:hello@1.0.0"
Target "wasi:cli/command"

Import {pkg: "prose:markup/dsl" macros: true open: true}

Export run
Def run Fn {}
  println(render-html(document))

Def document Doc [
  Heading 1 "Hello, Wavelet"
  Para ["A Markdown-class document that is also a "
        Bold "component" ". No new syntax — just a library."]
  Rule
  Para ["See " Link "the design" design-md "."]
]

// A reference-style link is just a binding (§10).
Def design-md "https://github.com/logaan/wavelet/blob/main/design.md"
```

```console
$ wavelet run hello.wvl
```

```html
<h1>Hello, Wavelet</h1>
<p>A Markdown-class document that is also a <strong>component</strong>. No new
syntax — just a library.</p>
<hr>
<p>See <a href="https://github.com/logaan/wavelet/blob/main/design.md">the
design</a>.</p>
```

Two reader rules carry the syntax. *Arity-driven reading* (`design.md` §2.4):
`Heading` has arity 2, so `Heading 1 "Hello, Wavelet"` consumes the `1` and the
string with no parentheses, and `Para` has arity 1, so it consumes the one list
that follows. *The attachment rule* (`design.md` §2.2): a `(` binds as a call
only when it abuts its head, so prose strings and bracketed lists float freely as
plain values. Notice the paragraph prose is split across two adjacent string
literals on two lines — newlines are whitespace, adjacent strings are just two
text elements in the run, and the renderer concatenates them. Source lines wrap
like prose.

= The library you assume (skip if you only want to write documents)

Nothing here is privileged. The data model is two WIT variants; the surface is
`DefMacro`s that expand to constructors of that data; the renderer is an ordinary
recursive function. All of it lives in the `prose:markup` package.

== The document is a value of a WIT type

```
DefType inline variant {
  text(string),                          // a literal run of prose
  strong(list<inline>),                  // **bold**
  emph(list<inline>),                    // *italic*
  mono(string),                          // `inline code`
  link({text: list<inline>, href: string}),
  image({alt: string, src: string}),
  linebreak,                             // a hard line break
}

DefType block variant {
  heading({level: u8, inline: list<inline>}),
  paragraph(list<inline>),
  quote(list<block>),
  bullets(list<list<block>>),            // each item is a list of blocks
  numbers(list<list<block>>),
  code({lang: string, lines: list<string>}),
  rule,
  raw-html(string),
}
```

#note[
  *On recursion at the boundary.* `inline` and `block` are self-referential, and
  WIT has no recursive types (`design.md` §6.2). That is a non-issue here because
  the tree is *built and rendered inside one component* — `hello.wvl` imports the
  renderer and only a `string` ever crosses a boundary. If you want a
  language-agnostic renderer instead (render this document with a Rust
  component), export the tree as `wavelet:meta`'s arena `tree` type — the markup
  tree is, after all, exactly a Wavelet form tree. Either way the core language is
  untouched.
]

== The surface is macros that expand to constructors

Because inline runs coerce bare strings to `text`, the macros are pure templating
— no logic, which is the point: the sugar is *only* reader rewriting.

```
DefMacro heading {level inline}
  Quasi heading({level: Unquote(level), inline: Unquote(inline)})

DefMacro para   {inline}        Quasi paragraph(Unquote(inline))
DefMacro bold   {inline}        Quasi strong(Unquote(inline))
DefMacro italic {inline}        Quasi emph(Unquote(inline))
DefMacro code   {s}            Quasi mono(Unquote(s))
DefMacro link   {inline href}   Quasi link({text: Unquote(inline), href: Unquote(href)})
DefMacro rule   {}             Quote rule
DefMacro break  {}             Quote linebreak
DefMacro doc    {blocks}        blocks            // the document is its block list
```

Their arities are exactly what the reader consumes:

#table(
  columns: (auto, auto, 1fr, auto),
  inset: 5pt, stroke: 0.4pt + luma(80%), align: (left, center, left, center),
  text(size: 8pt)[*Macro*], text(size: 8pt)[*Arity*],
  text(size: 8pt)[*Covers (core Markdown)*], text(size: 8pt)[*§*],
  [`Doc`], [1], [the document root], [1],
  [`Heading`], [2], [`#` … `######`], [3],
  [`Para`], [1], [a paragraph], [4],
  [`Bold` `Italic`], [1], [`**…**`, `*…*`], [5],
  [`Code`], [1], [`` `inline code` ``], [6],
  [`CodeBlock`], [2], [indented / fenced code], [6],
  [`Link` `Auto`], [2 / 1], [`[t](u)`, `<u>`], [7],
  [`Image`], [2], [`![alt](u)`], [8],
  [`BlockQuote`], [1], [`>` quotes], [9],
  [`Bullets` `Numbers` `Item`], [1], [`-`, `1.`, nesting], [10],
  [`Rule`], [0], [`---`], [4],
  [`Break`], [0], [hard line break], [5],
  [`Html`], [1], [inline / block raw HTML], [11],
)

== The renderer is an ordinary function

`render-html` is a plain recursion over the data — `Match` on each node, `map`
the children, `join` the strings. One representative arm:

```
Def render-block Fn {b}
  Match b [
    (heading({level: n inline: xs})
       str-cat("<h" to-string(n) ">" render-inline(xs) "</h" to-string(n) ">"))
    (paragraph(xs)   str-cat("<p>" render-inline(xs) "</p>"))
    (rule            "<hr>")
    (raw-html(s)     s)                    // passthrough, deliberately unescaped
    // … quote, bullets, numbers, code …
  ]

Def render-inline Fn {xs}
  // a lone string is a one-element run; everything else maps over the list
  join(map(coerce(xs) render-one) "")
```

`escape` (the function that turns `<` into `&lt;` for `text` and `mono` nodes but
not for `raw-html`) is forty lines of `str-cat` and lives here too — never in the
core. Equally, `prose:markup` could `Import` a Rust component that does the
rendering; `main.wvl` would not be able to tell (`design.md` §8).

= Headings

`Heading` takes a level and an inline run; the run may be a bare string or a list
with inline formatting in it.

#pair("ATX headings",
```md
# Title
## A subtitle
###### Deep
```
,
```
Heading 1 "Title"
Heading 2 "A subtitle"
Heading 6 "Deep"
```
)

#pair("Formatting inside a heading",
```md
## The *quick* brown fox
```
,
```
Heading 2 ["The " Italic "quick" " brown fox"]
```
)

= Paragraphs, breaks, and rules

A blank line between paragraphs in Markdown is two `Para` siblings in the block
list. A hard line break (Markdown's two trailing spaces) is `Break`; a thematic
break (`---`) is `Rule`.

#pair("Two paragraphs",
```md
First paragraph.

Second paragraph.
```
,
```
Para ["First paragraph."]
Para ["Second paragraph."]
```
)

#pair("Hard break + thematic break",
```md
Roses are red
Violets are blue

---
```
,
```
Para ["Roses are red" Break
      "Violets are blue"]
Rule
```
)

= Emphasis

`Bold` and `Italic` have arity 1, so they nest into each other and into runs
without any closing delimiter — the reader bounds them by the enclosing list.

#pair("Bold and italic",
```md
**bold**, *italic*, and
***both***
```
,
```
Para [Bold "bold" ", " Italic "italic"
      " and " Bold [Italic "both"]]
```
)

#pair("Emphasis mid-sentence",
```md
The *quick* and **the dead**.
```
,
```
Para ["The " Italic "quick" " and "
      Bold "the dead" "."]
```
)

= Code

Inline code is `Code` (a string). A code block is `CodeBlock lang body`, where
`body` is a list of lines — WAVE string literals are single-line, so a block is
written as its lines and the renderer joins them with newlines (which keeps the
source readable). Core Markdown's block syntax is a four-space indent, with no
language; `CodeBlock` carries an optional language string for free (pass `""` to
match Markdown exactly). Backticks, asterisks, and angle brackets inside code
need no escaping: it is all string data.

#pair("Inline code",
```md
Call `render-html` on `document`.
```
,
```
Para ["Call " Code "render-html"
      " on " Code "document" "."]
```
)

#pair("Indented block (Markdown) / lines (Wavelet)",
```md
    fn main() {
        println!("hi");
    }
```
,
```
CodeBlock "rust" [
  "fn main() {"
  "    println!(\"hi\");"
  "}"
]
```
)

= Links

Inline links are `Link text href`. Autolinks (`<https://…>`) are `Auto url`. A
*title* attribute is one more record field, so `Link` can also take the explicit
record form `Link "text" {href: "…" title: "…"}` — same macro, the renderer reads
the extra field. Reference-style links get a section of their own (§10) because
in a real language they are simply variables.

#pair("Inline link and autolink",
```md
[Anthropic](https://anthropic.com)
and <https://wavelet.dev>
```
,
```
Para [Link "Anthropic" "https://anthropic.com"
      " and " Auto "https://wavelet.dev"]
```
)

#pair("A link with formatted text",
```md
[the **bold** truth](/truth)
```
,
```
Para [Link ["the " Bold "bold" " truth"]
           "/truth"]
```
)

= Images

`Image alt src`. As in Markdown, an image is inline content, so it sits in a run
and can be wrapped in a `Link`.

#pair("Image",
```md
![a wavelet](/img/wave.png)
```
,
```
Para [Image "a wavelet" "/img/wave.png"]
```
)

#pair("Linked image",
```md
[![logo](/logo.png)](/home)
```
,
```
Para [Link [Image "logo" "/logo.png"]
           "/home"]
```
)

= Blockquotes

`BlockQuote` takes a *list of blocks*, so a quote can contain paragraphs, lists,
code — or another `BlockQuote`, which is how Markdown's `> >` nesting is spelled.

#pair("Nested, multi-block quote",
```md
> To be, or not to be.
>
> > — deeper still
```
,
```
BlockQuote [
  Para ["To be, or not to be."]
  BlockQuote [Para ["— deeper still"]]
]
```
)

= Lists

`Bullets` and `Numbers` take a list of `Item`s; each `Item` holds a list whose
elements are inline runs *or* blocks. Bare strings make a tight item; nest a
`Bullets`/`Numbers` for sub-lists; use `Para`s for a loose, multi-paragraph item.

#pair("Unordered, with a nested list",
```md
- Fruit
  - apple
  - pear
- Veg
```
,
```
Bullets [
  Item ["Fruit"
        Bullets [Item ["apple"]
                 Item ["pear"]]]
  Item ["Veg"]
]
```
)

#pair("Ordered, loose (paragraphs per item)",
```md
1. First step.

   More detail.
2. Second step.
```
,
```
Numbers [
  Item [Para ["First step."]
        Para ["More detail."]]
  Item [Para ["Second step."]]
]
```
)

#note[
  *Start number and ordered/unordered mixing.* `Numbers` counts from 1. To start
  elsewhere — Markdown's `3.`, `4.`, … — pass the record form
  `Numbers {start: 3 items: [ … ]}`; the same macro dispatches on whether it
  received a list or a record. The data model already carries everything core
  Markdown's list grammar can express; the surface is just which spelling reads
  best.
]

= Raw HTML and reference-style links

Two of core Markdown's features are, in Wavelet, not special cases of the markup
at all — they are features of the *language* the markup is embedded in.

*Inline / block HTML.* Markdown lets you drop raw HTML in. So does `Html`, which
passes its string through unescaped.

#pair("Raw HTML block",
```md
<details>
  <summary>More</summary>
  Hidden.
</details>
```
,
```
Html "<details><summary>More</summary>Hidden.</details>"
```
)

*Reference-style links.* Markdown invented `[text][id]` … `[id]: url` so a URL
could be named once and reused. In a real language that is just a binding — and
unlike Markdown, the "link table" is a value you can build, share, and compute
over.

#pair("Named once, used twice",
```md
See [the design][d] and the
[license][l].

[d]: https://wavelet.dev/design
[l]: https://wavelet.dev/license
```
,
```
Para ["See " Link "the design" design
      " and the " Link "license" license "."]

Def design  "https://wavelet.dev/design"
Def license "https://wavelet.dev/license"
```
)

= Escaping is (mostly) a non-problem

In Markdown the markup shares a channel with the prose, so `*`, `_`, #raw("`"),
`<`, `&`, `[` must be backslash-escaped when you mean them literally. In Wavelet
the prose is *quoted data* on its own channel; those characters are ordinary
text, and the renderer is what HTML-escapes on the way out. The only escapes you
ever write are WAVE's own string escapes (`\"`, `\\`, `\t`, `\u{2603}`).

#pair("Literal punctuation, no backslashes",
```md
A literal \*asterisk\*, an
ampersand \&, and 1 \< 2.
```
,
```
Para ["A literal *asterisk*, an "
      "ampersand &, and 1 < 2."]
```
)

= A full document

Putting the gallery together — a short post that reads, in source, like prose.

```
// post.wvl
Package "blog:release-0-2@1.0.0"

Export document
Def document Doc [
  Heading 1 "Wavelet 0.2 is out"
  Para ["This release makes the markup library " Bold "stable" ". "
        "Highlights below; see the " Link "changelog" changelog " for the rest."]

  Heading 2 "What changed"
  Bullets [
    Item ["Documents are " Italic "components" " — compose a renderer in."]
    Item ["Reference links are bindings:"
          CodeBlock "" ["Def home \"https://wavelet.dev\""]]
    Item ["Raw HTML still works, e.g. " Code "<kbd>"
          " via " Code "Html" "."]
  ]

  BlockQuote [Para ["“The document is the program.”"]]

  Heading 2 "Upgrade"
  Numbers [
    Item ["Bump " Code "prose:markup" " to 0.2."]
    Item ["Rebuild: " Code "wavelet build *.wvl" "."]
  ]
  Rule
  Para [Italic "Thanks to everyone who filed issues."]
]

Def changelog "https://wavelet.dev/changelog"
```

= What core Markdown can't do, and this gets for free

Parity with core Markdown was the bar; meeting it with a *real language* means
overshooting it without trying. Briefly, because it follows from the substrate
rather than from any added feature:

- *Abstraction.* A recurring construct — a callout, an admonition, a labelled
  figure — is one more `DefMacro`, indistinguishable at the call site from the
  built-ins. Markdown has no user-defined block.
- *Computed content.* The body of a document is an expression, so a table of
  contents, a sorted glossary, or a list rendered from data is `map`/`fold` over
  values, not a copy-paste chore:
  ```
  Bullets map(team Fn {m} Item [Link member-name(m) member-url(m)])
  ```
- *Transclusion.* `Import` another component's exported `document` and splice it
  in. Include files, shared headers, and partials are just composition
  (`design.md` §6.5) — no preprocessor.
- *One artifact, many renderers.* The same tree renders to HTML, to a terminal,
  to a slide deck, by composing a different renderer component — the document
  never mentions HTML.

= Ledger — the honest costs

In the spirit of `design.md`'s appendix:

- *You must import the manifest.* Paren-free TitleCase reading needs the macro
  arities in scope, so a document begins with one `Import … macros: true` line.
  That is the price of the sugar; without it, `Heading 1 "Hi"` does not know to
  consume two forms.
- *Prose is quoted.* Quoting every text run costs a pair of `"`s and makes very
  inline-dense paragraphs more punctuated than Markdown's. The payoff is the
  vanished escaping problem (§11) and prose that is unambiguously data.
- *The content tree is recursive.* It lives inside a component, or rides the
  `wavelet:meta` arena `tree` across boundaries (§2 note). Neither path touches
  the core — which is the whole claim: *core Markdown's expressive range, with
  zero changes to the language or its standard library.*
