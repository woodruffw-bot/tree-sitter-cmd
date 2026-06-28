; tree-sitter-cmd — language injection queries.
;
; cmd.exe scripts are frequently polyglots or embed other interpreters. The
; most reliable, low-false-positive injection is a FOR /F that captures the
; output of a `backquoted` command, which is itself a cmd command line.
((backquote_string) @injection.content
 (#set! injection.language "cmd"))
