; tree-sitter-cmd — language injection queries.
;
; cmd.exe scripts are frequently polyglots or embed other interpreters. The
; most reliable, low-false-positive injection is a FOR /F command source — the
; `` `backquoted` `` or 'single-quoted' command — which is itself a cmd command
; line. The offsets trim the surrounding quotes so the injected parser sees only
; the command text (not the delimiters).
((backquote_string) @injection.content
 (#set! injection.language "cmd")
 (#offset! @injection.content 0 1 0 -1))

((single_quote_string) @injection.content
 (#set! injection.language "cmd")
 (#offset! @injection.content 0 1 0 -1))
