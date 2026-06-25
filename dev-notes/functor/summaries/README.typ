// dev-notes/functor/summaries/README.typ — inter-step hand-off summaries.

#set document(title: "Functor plan — inter-step summaries")
#set page(paper: "a4", margin: (x: 2.1cm, y: 2.0cm))
#set text(size: 10pt)
#show raw: set text(font: "DejaVu Sans Mono", size: 8.5pt)
#set heading(numbering: none)

= Inter-step summaries

Each step in `../plan/` writes ONE summary here for the next step. A summary is
the trusted hand-off: the next agent reads it instead of redoing the previous
step's investigation. They are committed so the next agent sees them.

- `01-abi.typ`     — from step 01 (resource-export ABI spike)
- `02-bodies.typ`  — from step 02 (rep + core function bodies)
- `03-wiring.typ`  — from step 03 (wire into `emit_component`)
- `04-routing.typ` — from step 04 (`alias/op` routing + handle lift/lower)
- `05-parity.typ`  — from step 05 (parity test suite)
- `06-done.typ`    — from step 06 (downstream surfaces + the PR)

Keep each one concrete: exact names, signatures, `file:line` pointers, gotchas,
and the build state it leaves behind. Vague summaries force the next agent to
re-read the whole compiler — which is exactly what this structure avoids.
