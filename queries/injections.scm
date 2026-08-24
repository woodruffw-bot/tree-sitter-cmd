; FOR /F treats single quotes as command delimiters by default. With usebackq,
; backticks delimit commands and single quotes are literal. Capture the inner
; node so the Rust highlighter receives command text without either delimiter.

((for_statement
  option: (for_option
    (for_flag) @for.flag
    argument: (argument) @for.options)
  set: (for_set
    (backquote_string
      content: (backquote_content) @injection.content)))
 (#match? @for.flag "^/[fF]$")
 (#match? @for.options "(?i)(^|[ \t\"])usebackq([ \t\"]|\\^[ \t\"]|$)")
 (#set! injection.self))

; Keep an unfinished usebackq command injectable while it is being edited. An
; unterminated backquote consumes the rest of the line, so the FOR is an ERROR
; node until its closing delimiters and DO body are entered.
((ERROR
  (for_option
    (for_flag) @for.flag
    argument: (argument) @for.options)
  (backquote_string
    content: (backquote_content) @injection.content))
 (#match? @for.flag "^/[fF]$")
 (#match? @for.options "(?i)(^|[ \t\"])usebackq([ \t\"]|\\^[ \t\"]|$)")
 (#set! injection.self))

((for_statement
  option: (for_option
    (for_flag) @for.flag
    !argument)
  set: (for_set
    (single_quote_string
      content: (single_quote_content) @injection.content)))
 (#match? @for.flag "^/[fF]$")
 (#set! injection.self))

((for_statement
  option: (for_option
    (for_flag) @for.flag
    argument: (argument) @for.options)
  set: (for_set
    (single_quote_string
      content: (single_quote_content) @injection.content)))
 (#match? @for.flag "^/[fF]$")
 (#not-match? @for.options "(?i)(^|[ \t\"])usebackq([ \t\"]|\\^[ \t\"]|$)")
 (#set! injection.self))
