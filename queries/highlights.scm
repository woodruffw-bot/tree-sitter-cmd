; tree-sitter-cmd — syntax highlighting queries.
; Capture names follow the common tree-sitter highlight conventions.

; ---------------------------------------------------------------------------
; Comments
; ---------------------------------------------------------------------------
(rem_comment
  (keyword) @comment)
(colon_comment) @comment
(comment_text) @comment
(label_text) @comment

; ---------------------------------------------------------------------------
; Keywords
; ---------------------------------------------------------------------------
(keyword) @keyword
(not) @keyword
(condition_keyword) @keyword
(if_flag) @keyword
(for_flag) @keyword
(set_flag) @keyword

; ---------------------------------------------------------------------------
; Commands & labels
; ---------------------------------------------------------------------------
(command_name) @function
(label (label_name) @constant)
(goto_statement
  target: (label_reference
    name: (label_name) @constant))

; ---------------------------------------------------------------------------
; Expansions / variables
; ---------------------------------------------------------------------------
(variable) @variable
(delayed_variable) @variable
(variable_name) @variable
(parameter) @variable.parameter
(all_arguments) @variable.parameter
(parameter_tilde) @variable.parameter
(loop_variable_declaration) @variable.parameter
(loop_variable) @variable.parameter
(percent_literal) @constant

; ---------------------------------------------------------------------------
; Literals
; ---------------------------------------------------------------------------
(string) @string
(caret_quoted_string) @string
(escape_sequence) @string.escape
(file_descriptor) @number

; ---------------------------------------------------------------------------
; Operators & punctuation
; ---------------------------------------------------------------------------
(comparison_operator) @operator
(redirect_operator) @operator
(redirect_dup_operator) @operator
(quiet) @punctuation.special

[
  "&"
  "&&"
  "||"
  "|"
  "="
] @operator

[
  "("
  ")"
] @punctuation.bracket
