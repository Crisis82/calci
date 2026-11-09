# mitex Typst definitions

`specs/` is the Typst side of [MiTeX](https://github.com/mitex-rs/mitex) 0.2.4 (`@preview/mitex`, Apache-2.0, see `LICENSE`): the `mitex-scope` that LaTeX converted by the `mitex` crate is evaluated against.
Used when `math_font` is an OpenType math font (not `katex`).

Changed from upstream: `specs/latex/standard.typ` no longer imports `@preview/xarrow` (GPL-3.0-only); a small `xarrow` built on `math.stretch` replaces it; `kai` (removed from Typst) is replaced in the `\KaTeX` logo, and the `Vmatrix` delimiter is a single `‖`.
Symbol names that Typst has since renamed are fixed in calci (`RENAMED_SYMBOLS` in `src/tex_typst.rs`).
