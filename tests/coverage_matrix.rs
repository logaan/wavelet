//! Drift checks for `conformance/COVERAGE.md` (step 11 item 1).
//!
//! The coverage matrix records several cells as **GAP**: places where WIT
//! synthesis currently produces *invalid* WIT (or lets an invalid shape
//! through to a late failure). These tests pin that broken reality at the
//! cheap read → expand → synthesize level, so that the session which fixes a
//! gap is forced to update `conformance/COVERAGE.md` in the same change: when
//! a fix lands, the corresponding assertion here fails, and both it and the
//! matrix row must be flipped together.
//!
//! Only synthesis-level gaps are pinned — nothing here needs wasm-tools,
//! wasmtime, or the conformance harness. The "untested but working" rows in
//! the matrix are deliberately NOT given positive tests here; real coverage
//! for those belongs to step 11 item 5's fixtures, at which point the matrix
//! rows gain their citations.

use wavelet::{expand, read_file, wit};

/// Read → expand → synthesize the file's WIT world (the same path
/// `tests/type_system.rs::synth` uses).
fn synth(src: &str) -> Result<String, String> {
    let (arena, roots) = read_file(src).map_err(|e| e.to_string())?;
    let (arena, roots) = expand::expand_file(arena, &roots, None)?;
    wit::synthesize(&arena, &roots)
}

/// COVERAGE.md "variant — multi-payload case": GAP (item 2). A WIT variant
/// case takes at most one payload type, but a multi-payload `DefType` case is
/// accepted and synthesizes `zip(list<u32>, list<u32>)` — invalid WIT. When
/// item 2 lands (check-time rejection in both engines), this test fails:
/// update the matrix row to cite the new rejection test, then invert this
/// assertion or fold it into that test.
#[test]
fn multi_payload_variant_case_still_synthesizes_invalid_wit() {
    let out = synth(
        "Package \"audit:multi@0.1.0\"\n\
         DefType mylist [zip(list(u32) list(u32)) other]\n\
         Export {name: f}\n\
         Def f Fn {v: mylist} The mylist v\n",
    )
    .expect("multi-payload variant still passes synthesis (COVERAGE.md GAP item 2)");
    assert!(
        out.contains("zip(list<u32>, list<u32>)"),
        "multi-payload case no longer renders two payload types — item 2 landed? \
         Update conformance/COVERAGE.md (variant — multi-payload case) and this test.\n{out}"
    );
}

/// COVERAGE.md "record with %-escaped field names — express": GAP (item 5).
/// The lexer strips the `%` escape and synthesis never re-applies it, so an
/// authored record whose field names are WIT keywords synthesizes unparseable
/// WIT (`record: u32` instead of `%record: u32`).
#[test]
fn percent_escaped_field_authoring_still_drops_the_escape() {
    let out = synth(
        "Package \"audit:esc@0.1.0\"\n\
         DefType awkward {%record: u32 %list: string}\n\
         Export {name: f}\n\
         Def f Fn {v: awkward} The awkward v\n",
    )
    .expect("%-escaped authoring still passes synthesis (COVERAGE.md GAP item 5)");
    assert!(
        out.contains("record awkward { record: u32, list: string }"),
        "%-escaped record fields no longer render unescaped — the authoring gap closed? \
         Update conformance/COVERAGE.md (record with %-escaped field names — express) \
         and this test.\n{out}"
    );
}

/// COVERAGE.md "`tuple0` in a boundary signature": GAP (audit finding). An
/// empty tuple type annotation synthesizes `tuple<>`, which WIT forbids —
/// today the failure only surfaces at componentize time, as an internal
/// error. The intended fix is a deliberate check-time refusal.
#[test]
fn tuple0_signature_still_synthesizes_empty_tuple() {
    let out = synth(
        "Package \"audit:tupz@0.1.0\"\n\
         Export {name: zero}\n\
         Def zero Fn {} The tuple() tuple0()\n",
    )
    .expect("tuple0 signature still passes synthesis (COVERAGE.md GAP item 5)");
    assert!(
        out.contains("-> tuple<>"),
        "empty tuple signature no longer renders tuple<> — the gap closed? \
         Update conformance/COVERAGE.md (tuple0 in a boundary signature) and this test.\n{out}"
    );
}

/// COVERAGE.md "borrow in result / stored position": the deliberate
/// diagnostics cover the `Export {result: borrow(…)}` and `DefType` spellings,
/// but a `The borrow(<resource>)` result annotation escapes the check and
/// synthesizes `-> borrow<counter>` — invalid WIT that dies at componentize
/// as an internal error. GAP (item 5).
#[test]
fn the_borrow_result_annotation_still_escapes_the_checker() {
    let out = synth(
        "Package \"audit:bres@0.1.0\"\n\
         DefResource counter {\n\
           New: Fn {start: u32} cell-new(start)\n\
           value: Fn {self: counter} The u32 cell-get(self)\n\
         }\n\
         Export counter\n\
         Export {name: bad}\n\
         Def bad Fn {start: u32} The borrow(counter) counter(start)\n",
    )
    .expect("The borrow(…) result still passes synthesis (COVERAGE.md GAP item 5)");
    assert!(
        out.contains("-> borrow<counter>"),
        "borrow-in-result via The no longer synthesizes — the hole closed? \
         Update conformance/COVERAGE.md (borrow in result / stored position) and this test.\n{out}"
    );
}
