; tree-sitter-cmd — syntax highlighting queries.
; Capture names follow the common tree-sitter highlight conventions.

; ---------------------------------------------------------------------------
; Comments
; ---------------------------------------------------------------------------
(rem_comment) @comment
(colon_comment) @comment
(comment_text) @comment
(label_text) @comment

; ---------------------------------------------------------------------------
; Keywords
; ---------------------------------------------------------------------------
(keyword) @keyword
(not) @keyword.operator
(condition_keyword) @keyword.operator
(if_flag) @keyword
(for_option) @keyword

; ---------------------------------------------------------------------------
; Commands & labels
; ---------------------------------------------------------------------------
(command_name) @function
(label (label_name) @label)
(goto_statement target: (argument) @label)

; ---------------------------------------------------------------------------
; Expansions / variables
; ---------------------------------------------------------------------------
(variable) @variable
(delayed_variable) @variable
(variable_name) @variable
(parameter) @variable.parameter
(all_arguments) @variable.parameter
(parameter_tilde) @variable.parameter
(loop_variable) @variable.parameter
(percent_literal) @constant

; ---------------------------------------------------------------------------
; Literals
; ---------------------------------------------------------------------------
(string) @string
(backquote_string) @string.special
(escape_sequence) @string.escape
(file_descriptor) @number

; ---------------------------------------------------------------------------
; Operators & punctuation
; ---------------------------------------------------------------------------
(comparison_operator) @operator
(redirect_operator) @operator
(redirect_dup_operator) @operator
(quiet) @operator

[
  "&"
  "&&"
  "||"
  "|"
] @operator

[
  "("
  ")"
] @punctuation.bracket
