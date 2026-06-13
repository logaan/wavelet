" Vim syntax file for Wavelet (.wvl)
"
" Token classes mirror the Wavelet lexer (src/lexer.rs) and the shared Prism
" grammar used by the docs (docs/src/prism/wavelet.js). Keep the three in sync.
"
" Definition order matters: for matches that start at the same column Vim gives
" priority to the item defined last, so the more specific tokens come later.

if exists("b:current_syntax")
  finish
endif

syntax case match

" Punctuation: ( ) [ ] { } , : /
syntax match waveletPunctuation "[(){}\[\],:/]"

" int / float / inf / nan numbers (optionally negative)
syntax match waveletNumber "-\?\<\%(inf\|nan\|\d\+\%(\.\d\+\)\?\%([eE][+-]\?\d\+\)\?\)\>"

" Booleans and WAVE option/result constructors (keywords have top priority)
syntax keyword waveletBoolean true false
syntax keyword waveletConstant some none ok err

" Call heads: a (possibly %-escaped, possibly qualified) name attached, with no
" space, to ( [ or {.
syntax match waveletCallHead "%\?[A-Za-z][A-Za-z0-9_-]*\%(/%\?[A-Za-z0-9_-]\+\)\?\%([([{]\)\@="

" The alias side of a free-standing qualified reference (kv/get as a value).
syntax match waveletNamespace "%\?[A-Za-z][A-Za-z0-9_-]*\%(/\)\@="

" Record keys: name:
syntax match waveletProperty "%\?[A-Za-z][A-Za-z0-9_-]*\%(\s*:\)\@="

" TitleCase macro heads: leading capital, at least one lowercase, no hyphen.
syntax match waveletMacro "\<[A-Z][A-Za-z0-9]*[a-z][A-Za-z0-9]*\>"

" '.' char literals, with \n / \u{...} escapes
syntax match waveletChar "'\%(\\\%(u{[0-9a-fA-F]\+}\|.\)\|[^\\']\)'"

" "..." strings, with \n / \u{...} escapes
syntax match waveletEscape contained "\\\%(u{[0-9a-fA-F]\+}\|[nrt\\\"']\)"
syntax region waveletString start=+"+ skip=+\\"+ end=+"+ contains=waveletEscape

" // line comments and /// doc comments (//// is an ordinary comment)
syntax match waveletComment "//.*$"
syntax match waveletDocComment "///\%(/\)\@!.*$"

highlight default link waveletComment     Comment
highlight default link waveletDocComment  SpecialComment
highlight default link waveletString      String
highlight default link waveletEscape      SpecialChar
highlight default link waveletChar        Character
highlight default link waveletNumber      Number
highlight default link waveletBoolean     Boolean
highlight default link waveletConstant    Constant
highlight default link waveletMacro       Keyword
highlight default link waveletCallHead    Function
highlight default link waveletNamespace   Type
highlight default link waveletProperty    Label
highlight default link waveletPunctuation Delimiter

let b:current_syntax = "wavelet"
