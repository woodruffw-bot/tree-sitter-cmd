/**
 * tree-sitter-cmd — a grammar for Windows cmd.exe / batch (.bat, .cmd) scripts.
 *
 * Design notes live in GRAMMAR_DESIGN.md. The short version:
 *
 *   - cmd.exe has no whole-file grammar; each physical line is expanded, parsed
 *     and executed in turn. We model the whole file as one CST for tooling.
 *   - `extras` is `[ \t]` plus an invisible caret line-continuation (`^\n`,
 *     modelled like tree-sitter-bash's `\\\n`): it splices the next physical
 *     line without leaving a node, so a continued word is one word and a
 *     continued separator still separates. Newlines are significant statement
 *     terminators.
 *   - A "word" (command name / argument) is a run of adjacent fragments: bare
 *     text, quoted strings, caret escapes and expansions. Adjacency without
 *     whitespace is expressed with the zero-width external `_concat` token, so
 *     `C:\%ROOT%\bin` is one argument but `a b` is two.
 *   - Expansions (`%VAR%`, `!VAR!`, `%1`, `%*`, `%~dp0`, `%%i`, `%%`) are single
 *     tokens, so maximal munch prefers a real expansion over a stray `%`/`!`
 *     with no parser ambiguity, while a lone `%` still falls back to literal.
 *   - Keywords are case-insensitive and recognised by keyword extraction
 *     (`word: $._cmd_text`): a keyword matches only as a whole word, so `set`
 *     is a keyword but `setlocal` is a command, with no token precedence needed.
 *   - Command operators bind, lowest to highest: `&` < `||` < `&&` < `|`
 *     (matching ReactOS's `OpString` ordering).
 *
 * @author tree-sitter-cmd contributors
 * @license MIT
 */

/* eslint-disable no-undef */

// Keyword/bareword disambiguation uses keyword extraction (see the header
// note), so the PREC table below covers only command operators, not keywords.

// Operator precedences, lowest to highest binding.
const PREC = {
  SEQ: 1, // &
  OR: 2, // ||
  AND: 3, // &&
  PIPE: 4, // |
  COMMAND: 5, // a simple command greedily owns its argument tail
};

// Modifier letters for %~ / %%~ expansions (case-insensitive).
const TILDE_MODS = '[dpnxfsatzDPNXFSATZ]*';
// An optional `$ENV:` PATH-search clause inside a %~ expansion.
const PATH_SEARCH = '(?:\\$[A-Za-z_][^:\\r\\n]*:)?';
// A FOR loop variable name is a single character — cmd accepts almost anything,
// not just letters (e.g. `%%#`, `%%1`/`%%2` from `tokens=1,2`, `%%@`). Exclude
// only whitespace, `%`, the modifier sigils `~`/`*`, operators, `=` and quotes.
const FOR_VAR = '[^ \\t\\r\\n%~*&|<>()="]';

/** Build a case-insensitive regexp source for a literal word. */
function ciSource(word) {
  return word
    .split('')
    .map((ch) =>
      /[a-zA-Z]/.test(ch)
        ? `[${ch.toLowerCase()}${ch.toUpperCase()}]`
        : ch.replace(/[.*+?^${}()|[\]\\/]/g, '\\$&'),
    )
    .join('');
}

/** Case-insensitive keyword as a single token. */
function ci(word) {
  return token(new RegExp(ciSource(word)));
}

/**
 * Case-insensitive keyword aliased to the named `keyword` node so it appears in
 * the tree and is targetable by `(keyword)` highlight queries. Aliasing to the
 * *symbol* `$.keyword` (not the string `'keyword'`) is what makes it a named
 * node — hence `kw` takes `$`. (`rem` shares the same node via `$._rem`.)
 */
function kw($, word) {
  return alias(ci(word), $.keyword);
}

/**
 * A `/x` option flag (e.g. `/p`, `/a`, `/i`, `/f`). These appear only right
 * after their statement keyword and must win against a bareword in that slot
 * (e.g. `set /p` is the prompt flag, not a display of a variable named `/p`),
 * so they carry elevated token precedence.
 */
function opt(word) {
  return token(prec(3, new RegExp(ciSource(word))));
}

/**
 * A "word" (command name or argument): one lead fragment followed by zero or
 * more fragments joined by the zero-width external `_concat` token, so adjacent
 * fragments with no whitespace are one word (`C:\%ROOT%\bin`) while a space
 * splits them. `frag` defaults to `lead` where the lead and the fragments that
 * may follow it are drawn from the same set.
 */
function wordOf($, lead, frag = lead) {
  return seq(lead, repeat(seq($._concat, frag)));
}

/** A word in a slot where cmd treats `,`, `;`, and `=` as separators. */
function standardWordOf($, lead, frag = lead) {
  return seq(lead, repeat(seq($._standard_concat, frag)));
}

/** A redirection plus separators before the parser resumes its outer rule. */
function redirected($) {
  return seq(
    field('redirect', $._redirection),
    optional($._standard_separator),
  );
}

/**
 * Expansion spellings that must begin at the current byte. Duplication targets
 * use these aliases so global extras cannot turn a caret continuation into
 * spacing before a target.
 */
function immediateExpansion($) {
  return choice(
    alias(
      token.immediate(/%[^%0-9~*\r\n][^%\r\n]*%/),
      $.variable,
    ),
    alias(token.immediate(/![^!\r\n]+!/), $.delayed_variable),
    alias(token.immediate(/%[0-9]/), $.parameter),
    alias(token.immediate(/%\*/), $.all_arguments),
    alias(
      token.immediate(
        new RegExp('%~' + TILDE_MODS + PATH_SEARCH + '[0-9]'),
      ),
      $.parameter_tilde,
    ),
    alias(
      token.immediate(
        new RegExp('%%(?:~' + TILDE_MODS + PATH_SEARCH + ')?' + FOR_VAR),
      ),
      $.loop_variable,
    ),
    alias(token.immediate(/%%/), $.percent_literal),
  );
}

/** Attach `@` prefixes and skip separators before the command they suppress. */
function quietPrefix($) {
  return repeat(
    seq(
      field('quiet', $.quiet),
      optional($._standard_separator),
    ),
  );
}

/** A required binary operator, including its line-local missing-RHS path. */
function requiredOperator(
  $,
  operator,
  danglingOperator,
  continuedDanglingOperator,
  spelling,
) {
  return choice(
    seq(
      // Keep the normal spelling as a regex token. A string literal has higher
      // lexical priority than an equal-length external token and would hide the
      // source-backed dangling path at newline/EOF.
      alias(operator, spelling),
      field('right', $._statement),
    ),
    seq(
      alias(danglingOperator, spelling),
      alias($.body_boundary, '_body_boundary'),
      alias($.body_boundary_again, '_body_boundary'),
      field('right', alias($._command_start, 'command')),
    ),
    seq(
      alias(continuedDanglingOperator, spelling),
      alias($.body_boundary, '_body_boundary'),
      alias($.body_boundary_again, '_body_boundary'),
      field('right', alias($._command_start, 'command')),
    ),
    seq(
      alias(operator, spelling),
      alias($.body_boundary, '_body_boundary'),
      alias($.body_boundary_again, '_body_boundary'),
      field('right', alias($._command_start, 'command')),
    ),
  );
}

module.exports = grammar({
  name: 'cmd',

  externals: ($) => [
    $._concat,
    $._standard_concat,
    $._redirect_target_separator_ahead,
    $._rem,
    $._rem_text,
    $._redirect_source,
    $._block_open,
    $._block_close,
    $._lparen,
    $._rparen,
    $._caret_escape,
    $._string_end,
    $._set_inner_quote,
    $._set_string_end,
    $._set_ignored_suffix,
    $._label_leading_space,
    $._set_binding_end,
    $._dangling_and_operator,
    $._dangling_or_operator,
    $._dangling_pipe_operator,
    $._continued_dangling_and_operator,
    $._continued_dangling_or_operator,
    $._continued_dangling_pipe_operator,
    $.body_boundary,
    $.body_boundary_again,
    $._command_start,
    // Tree-sitter marks every external token valid during error recovery. Keep
    // this unused token last so the scanner can detect that state and decline
    // zero-width tokens that would otherwise prevent recovery from advancing.
    $._error_sentinel,
  ],

  // Keyword extraction: a keyword only matches when it spans an entire word.
  word: ($) => $._cmd_text,

  // These pure choices are exposed as transparent supertypes so queries can
  // target the category without adding wrapper nodes to the CST.
  supertypes: ($) => [$._expansion, $._redirection],

  // Whitespace and caret line-continuations interleave anywhere. The
  // continuation `^\n` is an anonymous, invisible extra (tree-sitter-bash treats
  // `\\\n` the same way): it never appears as a node, so it cannot glue words
  // that whitespace should split. `echo a ^\nb` is two arguments (the space
  // ends the first), while `echo a^\nb` is the single word `ab` (the join is
  // adjacency, handled by `_concat`). See GRAMMAR_DESIGN.md §4.1.
  extras: ($) => [/[ \t]/, token(/\^\r?\n/)],

  conflicts: ($) => [
    [$.for_option],
    [$.set_assignment],
    [$._set_prompt_body],
    [$.set_arith],
    [$.set_quoted],
    [$.set_display],
    [$.set_display, $._set_binding_name],
  ],

  rules: {
    // A batch file is a sequence of physical lines. Every line is terminated by
    // a newline except, optionally, the last one.
    program: ($) => seq(repeat($._line), optional($._line_content)),

    _line: ($) => seq(optional($._line_content), $._newline),

    // `colon_comment` reaches here through `_statement`, so it must not also be
    // a direct alternative (that would be two paths to the same node).
    _line_content: ($) => choice($._statement, $.label),

    _newline: ($) => token(/\r?\n/),

    // ---------------------------------------------------------------------
    // Statements and the command-operator ladder.
    // ---------------------------------------------------------------------
    // `colon_comment` is a statement so a `::` comment is accepted after an
    // operator, like the `& rem` form. cmd treats `::` as an inline comment that
    // runs to end of line, so `dir &:: note` is `dir` then a trailing comment.
    _statement: ($) =>
      choice(
        $._unit,
        $.seq_list,
        $.or_list,
        $.and_list,
        $.pipeline,
        $.colon_comment,
        $.powershell_comment,
      ),

    seq_list: ($) =>
      prec.left(
        PREC.SEQ,
        seq(
          field('left', $._statement),
          '&',
          optional(field('right', $._statement)),
        ),
      ),

    or_list: ($) =>
      prec.left(
        PREC.OR,
        seq(
          field('left', $._statement),
          requiredOperator(
            $,
            token(/\|\|/),
            $._dangling_or_operator,
            $._continued_dangling_or_operator,
            '||',
          ),
        ),
      ),

    and_list: ($) =>
      prec.left(
        PREC.AND,
        seq(
          field('left', $._statement),
          requiredOperator(
            $,
            token(/&&/),
            $._dangling_and_operator,
            $._continued_dangling_and_operator,
            '&&',
          ),
        ),
      ),

    pipeline: ($) =>
      prec.left(
        PREC.PIPE,
        seq(
          field('left', $._statement),
          requiredOperator(
            $,
            token(/\|/),
            $._dangling_pipe_operator,
            $._continued_dangling_pipe_operator,
            '|',
          ),
        ),
      ),

    // A `@` echo-suppress prefix may precede any command form, not just a
    // plain command (e.g. `@rem`, `@if`, `@echo off`). cmd accepts the prefix
    // stacked (`@@fc ...`), each `@` a redundant suppression, so allow a run.
    _unit: ($) =>
      seq(
        optional($._standard_separator),
        choice(
          $.command,
          $.block,
          $.rem_comment,
          $.if_statement,
          $.for_statement,
          $.goto_statement,
          $.call_statement,
          $.set_statement,
        ),
      ),

    // A parenthesised compound. Newlines inside act like `&`. Redirections may
    // appear before or after the block. The parentheses are supplied by the
    // external scanner (which tracks block vs literal-paren nesting), so
    // `echo (text)` is literal but `( echo a )` is a block.
    block: ($) =>
      prec.right(
        seq(
          quietPrefix($),
          repeat(redirected($)),
          alias($._block_open, '('),
          optional($._block_body),
          alias($._block_close, ')'),
          repeat(redirected($)),
        ),
      ),
    _block_body: ($) =>
      seq(
        repeat($._newline),
        $._line_content,
        repeat(seq(repeat1($._newline), $._line_content)),
        repeat($._newline),
      ),

    command: ($) =>
      prec.right(
        PREC.COMMAND,
        seq(
          quietPrefix($),
          repeat(redirected($)),
          field('name', $.command_name),
          repeat(
            choice(
              field('argument', $.argument),
              redirected($),
            ),
          ),
        ),
      ),

    // The `@` echo-suppress prefix.
    quiet: ($) => '@',

    // ---------------------------------------------------------------------
    // Control flow — IF
    // ---------------------------------------------------------------------
    // IF [/I] [NOT] <condition> <command> [ELSE <command>]
    if_statement: ($) =>
      prec.right(
        seq(
          quietPrefix($),
          kw($, 'if'),
          optional($._standard_separator),
          optional(
            seq(
              alias(opt('/i'), $.if_flag),
              optional($._standard_separator),
            ),
          ),
          optional(
            seq(
              alias(ci('not'), $.not),
              optional($._standard_separator),
            ),
          ),
          field('condition', $._if_condition),
          // cmd parses the rest of each branch through its operator ladder.
          // `prec.right` keeps a following ELSE attached to this IF.
          choice(
            field('consequence', $._statement),
            $._missing_consequence,
          ),
          optional(
            seq(
              kw($, 'else'),
              choice(
                field('alternative', $._statement),
                $._missing_alternative,
              ),
            ),
          ),
        ),
      ),

    _if_condition: ($) => choice($.comparison, $.unary_condition),

    comparison: ($) =>
      seq(
        field('left', $._if_operand),
        optional($._if_comparison_separator),
        field('operator', $.comparison_operator),
        optional($._standard_separator),
        field('right', $._if_operand),
      ),
    comparison_operator: ($) =>
      choice(
        '==',
        ci('equ'),
        ci('neq'),
        ci('lss'),
        ci('leq'),
        ci('gtr'),
        ci('geq'),
      ),

    unary_condition: ($) =>
      seq(
        field(
          'kind',
          alias(
            choice(ci('exist'), ci('defined'), ci('errorlevel'), ci('cmdextversion')),
            $.condition_keyword,
          ),
        ),
        optional($._standard_separator),
        field('argument', $._if_operand),
      ),

    // An IF operand is a word whose bare text stops at `=` so that `a==b`
    // tokenises as `a`, `==`, `b`. It may also be fully wrapped in parentheses,
    // the classic `if (%1)==()` idiom for tolerating empty/odd arguments.
    _if_operand: ($) =>
      choice(
        alias($._if_word, $.argument),
        alias($._parenthesized_if_operand, $.argument),
      ),
    _parenthesized_if_operand: ($) =>
      prec(1, seq($._lparen, optional($._if_word), $._rparen)),
    _if_word: ($) => standardWordOf($, $._if_fragment),
    _if_fragment: ($) =>
      choice(
        alias($._if_text, $.text),
        $.string,
        $.escape_sequence,
        // A lone caret escaping a following `%`/`!` (e.g. `if ^%V:~0,1% …`).
        alias($._caret_escape, $.escape_sequence),
        $._expansion,
        alias($._stray_sigil, $.text),
        alias($._lparen, $.text),
        alias($._rparen, $.text),
      ),
    _if_text: ($) => token(/[^ \t\r\n&|<>()^"%!,;=]+/),

    // ---------------------------------------------------------------------
    // Control flow — FOR
    // ---------------------------------------------------------------------
    // FOR [/D | /R [path] | /L | /F ["opts"]] %%v IN (set) DO <command>
    for_statement: ($) =>
      prec.right(
        seq(
          quietPrefix($),
          kw($, 'for'),
          optional($._standard_separator),
          optional(
            seq(
              field('option', $.for_option),
              optional($._standard_separator),
            ),
          ),
          field('variable', $._loop_variable_declaration),
          optional($._standard_separator),
          kw($, 'in'),
          optional($._standard_separator),
          alias($._block_open, '('),
          field('set', optional($.for_set)),
          alias($._block_close, ')'),
          optional($._standard_separator),
          kw($, 'do'),
          // Operators after DO remain inside the loop body.
          choice(
            field('body', $._statement),
            $._missing_for_body,
          ),
        ),
      ),

    // An absent controller body retains only Tree-sitter's anonymous MISSING
    // command in the CST. The boundary markers are implementation terminals.
    _missing_consequence: ($) =>
      seq(
        alias($.body_boundary, '_body_boundary'),
        alias($.body_boundary_again, '_body_boundary'),
        field('consequence', alias($._command_start, 'command')),
      ),
    _missing_alternative: ($) =>
      seq(
        alias($.body_boundary, '_body_boundary'),
        alias($.body_boundary_again, '_body_boundary'),
        field('alternative', alias($._command_start, 'command')),
      ),
    _missing_for_body: ($) =>
      seq(
        alias($.body_boundary, '_body_boundary'),
        alias($.body_boundary_again, '_body_boundary'),
        field('body', alias($._command_start, 'command')),
      ),

    // FOR options. `/R [path]` and `/F [options]` each take one optional word.
    // `/D` and `/R` may be combined in either order. No other mixed switch set
    // is valid. The optional argument cannot start with `/`, which keeps an
    // illegal second switch from being accepted as an `/F` option or `/R` path.
    // The argument may be quoted (`/f "tokens=2 delims=,"`) or caret-escaped and
    // unquoted (`/f tokens^=2-5^ delims^=.-_`).
    for_option: ($) =>
      choice(
        alias(opt('/l'), $.for_flag),
        seq(
          alias(opt('/f'), $.for_flag),
          optional($._standard_separator),
          optional(field('argument', alias($._for_arg, $.argument))),
        ),
        seq(
          alias(opt('/d'), $.for_flag),
          optional($._standard_separator),
          optional(
            seq(
              alias(opt('/r'), $.for_flag),
              optional($._standard_separator),
              optional(field('argument', alias($._for_arg, $.argument))),
            ),
          ),
        ),
        seq(
          alias(opt('/r'), $.for_flag),
          optional($._standard_separator),
          optional(
            choice(
              seq(
                alias(opt('/d'), $.for_flag),
                optional($._standard_separator),
                optional(field('argument', alias($._for_arg, $.argument))),
              ),
              seq(
                field('argument', alias($._for_arg, $.argument)),
                optional(
                  seq(
                    optional($._standard_separator),
                    alias(opt('/d'), $.for_flag),
                  ),
                ),
              ),
            ),
          ),
        ),
      ),

    _for_arg: ($) =>
      standardWordOf($, $._for_arg_lead, $._standard_fragment),
    _for_arg_lead: ($) =>
      choice(
        alias($._for_arg_text, $.text),
        $.string,
        $.escape_sequence,
        alias($._caret_escape, $.escape_sequence),
        $._expansion,
        alias($._stray_sigil, $.text),
        alias($._lparen, $.text),
        alias($._rparen, $.text),
      ),
    _for_arg_text: ($) =>
      token(/[^/,;= \t\r\n&|<>()^"%!][^,;= \t\r\n&|<>()^"%!]*/),

    for_set: ($) =>
      repeat1(
        choice(
          alias($._standard_argument, $.argument),
          $.backquote_string,
          $.single_quote_string,
          $._standard_separator,
          $._newline,
        ),
      ),

    // A backquoted FOR /F item. It is a command only with `usebackq` and may be
    // unterminated. Keep the content in a neutral, delimiter-free child. The
    // injection query assigns command semantics only when this quote mode is
    // active.
    backquote_string: ($) =>
      seq(
        token(prec(2, '`')),
        optional(field('content', $.backquote_content)),
        optional(token.immediate(prec(3, '`'))),
      ),
    backquote_content: ($) => token.immediate(prec(4, /[^`\r\n]+/)),

    // A single-quoted FOR /F item. It is a command unless `usebackq` is active.
    // The closing quote is required so a stray apostrophe in a plain FOR set
    // (`for %%a in (it's)`) stays text. Double-quoted spans may contain literal
    // apostrophes, as in embedded PowerShell and Python snippets. Newlines are
    // also accepted because cmd permits a FOR /F command source to span lines.
    // As with backquotes, the content node is neutral until a query applies the
    // active quote mode.
    single_quote_string: ($) =>
      seq(
        token(prec(2, "'")),
        optional(field('content', $.single_quote_content)),
        token.immediate("'"),
      ),
    single_quote_content: ($) =>
      repeat1(
        choice(
          token.immediate(prec(4, /[^'"^\r\n]+/)),
          token.immediate(prec(4, /"[^"\r\n]*"/)),
          token.immediate(prec(4, /\^[^\r\n]/)),
          $._newline,
        ),
      ),

    // ---------------------------------------------------------------------
    // GOTO / CALL
    // ---------------------------------------------------------------------
    // GOTO label  /  GOTO :EOF. The target is the rest of the command up to a
    // cmd operator or redirection. Spaces may be part of a label name. A colon
    // or standard separator ends the lookup name, but its ignored suffix stays
    // in the CST as label text. cmd removes redirections before interpreting
    // that target, so preserve them before the keyword and around the target.
    goto_statement: ($) =>
      prec.right(
        seq(
          quietPrefix($),
          repeat(redirected($)),
          kw($, 'goto'),
          optional($._standard_separator),
          repeat(redirected($)),
          field('target', $.label_reference),
          repeat(redirected($)),
        ),
      ),

    label_reference: ($) =>
      choice(
        seq(
          optional($._label_reference_prefix),
          field(
            'name',
            alias($._label_reference_name, $.label_name),
          ),
          optional(alias($._label_reference_tail, $.label_text)),
        ),
        // Keep an empty or doubly-prefixed target, such as `goto ::name`, as a
        // target without inventing a resolvable label name.
        seq(
          ':',
          optional(alias($._label_reference_tail, $.label_text)),
        ),
      ),
    _label_reference_prefix: ($) =>
      seq(':', optional(token.immediate(/[ \t]+/))),
    _label_reference_name: ($) =>
      repeat1(
        choice(
          $._label_reference_text,
          $.escape_sequence,
          alias($._caret_escape, $.escape_sequence),
          $._expansion,
          $._stray_sigil,
        ),
      ),
    _label_reference_text: ($) =>
      token(
        /[^ \t\r\n:^+;,=&|<>()%!](?:[^:\r\n^+;,=&|<>()%!]*[^ \t:\r\n^+;,=&|<>()%!])?/,
      ),
    _label_reference_tail: ($) => token(/[:+;,=][^\r\n&|<>)]*/),

    // CALL :label args  /  CALL file args  /  CALL command. Redirections may
    // precede the keyword or appear anywhere in the argument tail (e.g.
    // `>log call "%~f0" <input %*`).
    call_statement: ($) =>
      prec.right(
        seq(
          quietPrefix($),
          repeat(redirected($)),
          kw($, 'call'),
          repeat(redirected($)),
          field('target', $.argument),
          repeat(
            choice(
              field('argument', $.argument),
              redirected($),
            ),
          ),
        ),
      ),

    // ---------------------------------------------------------------------
    // SET family
    // ---------------------------------------------------------------------
    set_statement: ($) =>
      prec.right(
        seq(
          quietPrefix($),
          repeat(redirected($)),
          kw($, 'set'),
          // Redirections before the payload remain statement children. Once a
          // SET branch starts, that branch keeps later redirections in source
          // order with the surrounding payload fragments.
          optional(
            choice(
              $._set_payload,
              seq(
                repeat1(field('redirect', $._redirection)),
                $._set_payload,
              ),
            ),
          ),
          // Keep terminal redirections on the statement, matching the existing
          // CST. Branch rules consume a redirection only when more payload
          // follows it.
          repeat(field('redirect', $._redirection)),
        ),
      ),
    _set_payload: ($) =>
      choice(
        $.set_prompt,
        $.set_arith,
        $.set_assignment,
        $.set_quoted,
        $.set_display,
      ),

    // SET name=value  (the value is the rest of the logical line)
    set_assignment: ($) =>
      seq(
        field(
          'name',
          alias($._set_binding_name, $.variable_name),
        ),
        '=',
        repeat(
          choice(
            field('value', $.argument),
            seq(
              repeat1(field('redirect', $._redirection)),
              field('value', $.argument),
            ),
          ),
        ),
      ),
    // SET /P [name]=prompt. The name may be empty (the `<nul set /p=text` trick
    // for printing without a trailing newline). cmd also accepts the prompt in
    // the quoted `"name=prompt"` form (so the prompt can hold spaces, e.g.
    // `set /p "answer=Enter choice: "`); the quoted span runs first-quote to
    // last-quote like SET "x=y", so model that branch like set_quoted.
    set_prompt: ($) =>
      prec.right(
        seq(
          alias(opt('/p'), $.set_flag),
          repeat(field('redirect', $._redirection)),
          $._set_prompt_body,
        ),
      ),
    _set_prompt_body: ($) =>
      choice(
        seq(
          optional(
            field(
              'name',
              alias($._set_binding_name, $.variable_name),
            ),
          ),
          '=',
          repeat(
            choice(
              field('prompt', $.argument),
              seq(
                repeat1(field('redirect', $._redirection)),
                field('prompt', $.argument),
              ),
            ),
          ),
        ),
        seq(
          '"',
          optional(
            field('name', alias($._set_name, $.variable_name)),
          ),
          '=',
          optional(
            field(
              'prompt',
              alias($._set_quoted_value, $.argument),
            ),
          ),
          $._set_string_end,
          optional($._set_ignored_tail),
        ),
      ),
    // SET /A expression  (refined to an arithmetic sub-grammar in M7)
    set_arith: ($) =>
      seq(
        alias(opt('/a'), $.set_flag),
        repeat(
          choice(
            field('expression', $.argument),
            seq(
              repeat1(field('redirect', $._redirection)),
              field('expression', $.argument),
            ),
          ),
        ),
      ),
    // SET "name=value". Keep the binding fields visible instead of hiding the
    // assignment in a generic string. Earlier quotes remain value text and the
    // SET-specific terminator leaves the final quote as the wrapper close.
    // Caret-escaped wrapper quotes are also common in macro definitions. They
    // remain opaque because their caret and quote phase ordering differs from
    // ordinary quoted assignments.
    set_quoted: ($) =>
      choice(
        seq(
          '"',
          optional(field('name', alias($._set_name, $.variable_name))),
          '=',
          optional(
            field('value', alias($._set_quoted_value, $.argument)),
          ),
          $._set_string_end,
          optional($._set_ignored_tail),
        ),
        prec.right(
          seq(
            $.caret_quoted_string,
            repeat(
              choice(
                $._fragment,
                seq(
                  repeat1(field('redirect', $._redirection)),
                  $._fragment,
                ),
              ),
            ),
          ),
        ),
      ),
    _set_quoted_value: ($) =>
      prec.right(
        1,
        repeat1(
          choice(
            alias($._string_text, $.text),
            $._expansion,
            alias($._string_sigil, $.text),
            alias($._set_inner_quote, $.text),
          ),
        ),
      ),
    _set_ignored_tail: ($) =>
      repeat1(
        choice(
          field(
            'ignored',
            alias($._set_ignored_suffix, $.set_ignored_suffix),
          ),
          seq(
            repeat1(field('redirect', $._redirection)),
            field(
              'ignored',
              alias($._set_ignored_suffix, $.set_ignored_suffix),
            ),
          ),
        ),
      ),
    caret_quoted_string: ($) =>
      token(/\^"(?:[^\r\n^]|\^[^"\r\n])*(?:\^"|")/),
    // SET  /  SET prefix  (display). Redirections can split an unquoted query;
    // retain each surviving segment so tools can reconstruct the cmd input.
    // The quoted spelling keeps the same last-quote wrapper rule as an
    // assignment, and cmd strips that wrapper before taking the display path.
    set_display: ($) =>
      choice(
        seq(
          alias($._set_name, $.variable_name),
          repeat(
            seq(
              repeat1(field('redirect', $._redirection)),
              alias($._set_name, $.variable_name),
            ),
          ),
        ),
        seq(
          '"',
          optional(alias($._set_name, $.variable_name)),
          $._set_string_end,
          optional($._set_ignored_tail),
        ),
      ),

    // Redirections are removed before SET interprets its payload. Keep a
    // logical name as one node even when source redirections split its text.
    // The assignment form also permits a final redirect immediately before
    // `=`. The zero-width end token verifies that the next non-horizontal byte
    // really is `=`, so a display's terminal redirect is not recovered as an
    // assignment with a missing delimiter.
    _set_binding_name: ($) =>
      seq(
        $._set_name,
        repeat(
          seq(
            repeat1(field('redirect', $._redirection)),
            choice(
              $._set_name,
              $._set_binding_end,
            ),
          ),
        ),
      ),

    // A SET variable name stops at `=` and may contain expansions (for example
    // `%~1r` or `_nt!nt!`) and internal spaces. Keep expansions as children of
    // `variable_name` so a static analyzer can recover the computed binding.
    // A leading plain-text fragment is non-space, which prevents the separator
    // after SET from becoming part of the name. Text after a special fragment
    // may begin with whitespace so the complete name range reaches `=`. `/a`
    // and `/p` still win their slot through token precedence.
    _set_name: ($) =>
      choice(
        $._set_name_text,
        prec.right(
          seq(
            repeat(alias($._set_name_text, $.text)),
            $._set_name_special,
            repeat($._set_name_fragment),
          ),
        ),
      ),
    _set_name_special: ($) =>
      choice(
        $.escape_sequence,
        alias($._caret_escape, $.escape_sequence),
        $._expansion,
        alias($._stray_sigil, $.text),
      ),
    _set_name_fragment: ($) =>
      choice(
        alias($._set_name_tail_text, $.text),
        $._set_name_special,
      ),
    _set_name_text: ($) =>
      token(/[^ \t\r\n&|<>()^"%!=][^\r\n&|<>()^"%!=]*/),
    // Once a special fragment has established that this is a compound name,
    // adjacent text may begin with whitespace. Keeping this token immediate
    // prevents trailing name whitespace before `=` from being consumed as an
    // extra and lost from the `variable_name` range.
    _set_name_tail_text: ($) =>
      token.immediate(/[^\r\n&|<>()^"%!=]+/),

    // ---------------------------------------------------------------------
    // Redirections
    // ---------------------------------------------------------------------
    _redirection: ($) => choice($.redirect_file, $.redirect_dup),

    // `>`, `>>`, `<`, with an optional immediately-preceding source fd digit.
    // The external source token only matches a digit whose next byte is the
    // operator. The immediate operator branch then preserves that adjacency.
    redirect_file: ($) =>
      choice(
        seq(
          field(
            'source',
            alias($._redirect_source, $.file_descriptor),
          ),
          field(
            'operator',
            alias(token.immediate(/>>|>|</), $.redirect_operator),
          ),
          $._redirect_file_target,
        ),
        seq(
          field('operator', $.redirect_operator),
          $._redirect_file_target,
        ),
      ),
    redirect_operator: ($) => token(/>>|>|</),
    _redirect_file_target: ($) =>
      choice(
        field('target', $.argument),
        seq(
          $._redirect_target_separator_ahead,
          field('target', alias($._standard_argument, $.argument)),
        ),
        seq(
          $._standard_separator,
          field('target', alias($._standard_argument, $.argument)),
        ),
      ),

    // Handle duplication: `2>&1`, `>&2`, `<&3`. cmd skips its standard
    // separators before the target, but this parser phase does not treat a
    // caret-newline as such a separator. Keep both the spacing and target
    // immediate so the global continuation extra cannot bridge that boundary.
    redirect_dup: ($) =>
      choice(
        seq(
          field(
            'source',
            alias($._redirect_source, $.file_descriptor),
          ),
          field(
            'operator',
            alias(token.immediate(/[<>]&/), $.redirect_dup_operator),
          ),
          $._redirect_dup_target,
        ),
        seq(
          field('operator', $.redirect_dup_operator),
          $._redirect_dup_target,
        ),
      ),
    _redirect_dup_target: ($) =>
      seq(
        optional($._redirect_dup_spacing),
        field('target', $._redirect_dup_valid_target),
      ),
    _redirect_dup_valid_target: ($) =>
      choice(
        alias(token.immediate(/[0-9]/), $.file_descriptor),
        immediateExpansion($),
      ),
    _redirect_dup_spacing: ($) => token.immediate(/[ \t,;=]+/),
    redirect_dup_operator: ($) => token(/[<>]&/),
    file_descriptor: ($) => token.immediate(/[0-9]/),

    // ---------------------------------------------------------------------
    // Words: command names and arguments are runs of adjacent fragments.
    // ---------------------------------------------------------------------
    command_name: ($) =>
      standardWordOf($, $._cmd_lead, $._standard_fragment),

    // The first fragment of a command name may not begin with `@` (quiet) or
    // `:` (label). A lone `%`/`!` sigil can also lead a name: cmd happily parses
    // `! echo ...` as a command named `!` (it fails at runtime — a common
    // debug-disable trick), and `%`/`!` only form an expansion when they pair up.
    _cmd_lead: ($) =>
      choice(
        $._cmd_path,
        $._cmd_punct_lead,
        $._cmd_text,
        $.string,
        $._expansion,
        alias($._stray_sigil, $.text),
      ),
    // Preserve dotted executable names and explicit paths as one command-name
    // token. This keeps names such as `if.exe` from being mistaken for an
    // internal command when `_cmd_text` stops at cmd's keyword delimiters.
    _cmd_path: ($) =>
      token(
        prec(
          1,
          choice(
            /[^ \t\r\n&|<>()^"%!@:,/;=\[\]\\]+[.\\][^ \t\r\n&|<>()^"%!,;=]*/,
            /\.{1,2}[\\/][^ \t\r\n&|<>()^"%!,;=]*/,
            /\\\\[^ \t\r\n&|<>()^"%!,;=]*/,
          ),
        ),
      ),
    // A generic command may itself begin with one of cmd's internal-command
    // delimiters. Keep that fallback without letting the same token swallow a
    // delimiter after a recognized keyword such as `set/a`.
    _cmd_punct_lead: ($) =>
      token(/[.,/;=\[\]\\][^ \t\r\n&|<>()^"%!,;=]*/),
    // cmd ends an internal-command name at `:.\,/;=[]`, so `goto:eof` is
    // `goto` + `:eof` and `set/a` is `set` + `/a`. Explicit paths and dotted
    // executable names are handled by `_cmd_path`, while a leading drive letter
    // remains one `_cmd_text` token.
    _cmd_text: ($) =>
      token(
        choice(
          /[A-Za-z]:[^ \t\r\n&|<>()^"%!,;=]*/,
          /[^ \t\r\n&|<>()^"%!@:,./;=\[\]\\][^ \t\r\n&|<>()^"%!:,./;=\[\]\\]*/,
        ),
      ),

    // `=` remains a word boundary in the scanner so IF `a==b` and SET
    // `name=value` can recognize their delimiters. In an ordinary argument,
    // however, an immediately-adjacent `=` continues the current word. The
    // immediate branch joins it without changing the scanner's global rule.
    argument: ($) =>
      seq(
        $._fragment,
        repeat(
          choice(
            seq($._concat, $._fragment),
            alias(
              token.immediate(/=[^ \t\r\n&|<>()^"%!]*/),
              $.text,
            ),
          ),
        ),
      ),

    // cmd skips `,`, `;`, and `=` when fetching tokens at grammar
    // boundaries. This word form is used only in those slots. The ordinary
    // `argument` rule keeps the same punctuation literal in command tails.
    _standard_argument: ($) => standardWordOf($, $._standard_fragment),
    _standard_fragment: ($) =>
      choice(
        alias($._standard_text, $.text),
        $.string,
        $.escape_sequence,
        alias($._caret_escape, $.escape_sequence),
        $._expansion,
        alias($._stray_sigil, $.text),
        alias($._lparen, $.text),
        alias($._rparen, $.text),
      ),
    _standard_text: ($) => token(/[^ \t\r\n&|<>()^"%!,;=]+/),

    // These stay hidden in the CST, like spaces in the same parser slots. A
    // token covers the full run, including interleaved spaces/continuations,
    // so an optional separator does not make following CST fields repeatable.
    // Lexical precedence keeps a leading run out of `_cmd_punct_lead`.
    _standard_separator: ($) =>
      token(prec(2, /[,;=](?:(?:[ \t]|\^\r?\n)*[,;=])*/)),
    _if_comparison_separator: ($) =>
      token(prec(2, /[,;](?:(?:[ \t]|\^\r?\n)*[,;])*/)),

    _fragment: ($) =>
      choice(
        $.text,
        $.string,
        $.escape_sequence,
        // A lone caret escaping a following `%`/`!` expansion (`echo ^%PATH^%`).
        alias($._caret_escape, $.escape_sequence),
        $._expansion,
        // A `%` or `!` that is not part of an expansion is literal text.
        alias($._stray_sigil, $.text),
        // Literal parentheses in argument position (the scanner only emits
        // these where a block cannot begin), e.g. `echo (text)`.
        alias($._lparen, $.text),
        alias($._rparen, $.text),
      ),

    text: ($) => token(/[^ \t\r\n&|<>()^"%!]+/),
    _stray_sigil: ($) => token(/[%!]/),

    // A caret escapes the single following character — but NOT a `%`/`!` that
    // begins an expansion: in cmd `^%VAR%` expands `%VAR%` first and the caret
    // escapes the *result*, so the caret must not swallow the opening sigil (or
    // the now-unbalanced `%`/`!` makes a later one match greedily across the
    // line). So `^&`, `^"`, `^^`, `^)` escape their char here, while a caret
    // before `%`/`!` is a lone `_caret_escape` (external) followed by the
    // expansion, and `^`-newline is the line-continuation extra.
    escape_sequence: ($) => token(/\^[^\r\n%!]/),

    // cmd does not strip quotes; they only group, and `%VAR%`/`!VAR!` still
    // expand inside them, so the interior is sub-noded: literal text interleaved
    // with the same expansion forms used elsewhere. A quoted `%PATH%` is a real
    // `variable` node, while the surrounding literal text stays hidden (the
    // `string` node covers it). The terminator is the external `_string_end`:
    // the scanner consumes the closing `"`, or matches zero-width at end of line
    // / end of input so an unterminated quote still closes (cmd runs it to EOL).
    // Using an explicit terminator instead of an optional `"` avoids a lone `"`
    // parsing as a degenerate empty string, which would let `"%PATH%"` split
    // into two empty strings around a bare expansion. The literal-text and lone
    // `%`/`!` sigil tokens are `token.immediate` so interior spaces stay part of
    // the string rather than being skipped as whitespace extras.
    string: ($) => seq('"', repeat($._string_part), $._string_end),
    _string_part: ($) =>
      choice($._string_text, $._expansion, $._string_sigil),
    _string_text: ($) => token.immediate(/[^"%!\r\n]+/),
    _string_sigil: ($) => token.immediate(/[%!]/),

    // ---------------------------------------------------------------------
    // Expansions (each is a single token; see header note)
    // ---------------------------------------------------------------------
    _expansion: ($) =>
      choice(
        $.variable,
        $.delayed_variable,
        $.parameter,
        $.all_arguments,
        $.parameter_tilde,
        $.loop_variable,
        alias($._loop_variable_modified, $.loop_variable),
        $.percent_literal,
      ),

    // %NAME% and %NAME:...% (substring / substitution). The leading character
    // is not a digit/`~`/`*` so positional, tilde and `%*` forms win instead.
    variable: ($) => token(/%[^%0-9~*\r\n][^%\r\n]*%/),
    // !NAME! delayed expansion (always recognised; literal unless delayed
    // expansion is enabled at runtime).
    delayed_variable: ($) => token(/![^!\r\n]+!/),

    // %0..%9 positional parameters.
    parameter: ($) => token(/%[0-9]/),
    // %* all command-line arguments.
    all_arguments: ($) => token(/%\*/),
    // %~modifiers[$ENV:]N argument modifiers, e.g. %~dp0, %~$PATH:1.
    parameter_tilde: ($) =>
      token(new RegExp('%~' + TILDE_MODS + PATH_SEARCH + '[0-9]')),
    // The unmodified token is shared by references and declarations. The
    // binder position aliases it to `loop_variable_declaration`, while the
    // modified form is reachable only through `_expansion` as a reference.
    _loop_variable_declaration: ($) =>
      alias($.loop_variable, $.loop_variable_declaration),
    loop_variable: ($) => token(new RegExp('%%' + FOR_VAR)),
    _loop_variable_modified: ($) =>
      token(new RegExp('%%~' + TILDE_MODS + PATH_SEARCH + FOR_VAR)),
    // %% literal percent sign.
    percent_literal: ($) => token(/%%/),

    // ---------------------------------------------------------------------
    // Labels and comments
    // ---------------------------------------------------------------------
    label: ($) =>
      prec(
        2,
        seq(
          quietPrefix($),
          token(/:/),
          optional($._label_leading_space),
          field('name', alias($._label_name, $.label_name)),
          optional(alias($._label_tail, $.label_text)),
        ),
      ),
    _label_name: ($) =>
      token.immediate(
        prec(
          2,
          /[^ \t\r\n:+;,=&|<>()](?:[^:\r\n+;,=&|<>()]*[^ \t:\r\n+;,=&|<>()])?/,
        ),
      ),
    _label_tail: ($) => token(/[^\r\n]+/),

    // Keep the body opaque. Expansion-shaped text inside REM is comment text,
    // not an expansion that tooling should treat as live code.
    rem_comment: ($) =>
      seq(
        quietPrefix($),
        repeat(redirected($)),
        alias($._rem, $.keyword),
        optional(alias($._rem_text, $.comment_text)),
      ),
    colon_comment: ($) =>
      prec(
        1,
        seq(
          quietPrefix($),
          choice(
            seq(token(/::/), optional(alias($._line_text, $.comment_text))),
            // A colon at command position consumes the rest of that physical
            // line. At top level, the higher-precedence `label` rule wins for
            // a valid label definition.
            seq(token(/:/), optional(alias($._line_text, $.comment_text))),
          ),
        ),
      ),
    // Batch and PowerShell polyglots use these markers so PowerShell sees a
    // block comment while cmd reaches the batch section. Keep marker lines
    // opaque so their punctuation does not trigger redirection recovery.
    powershell_comment: ($) =>
      choice(
        seq(token(/<#/), optional(alias($._line_text, $.comment_text))),
        token(/#>[^\r\n]*/),
      ),
    _line_text: ($) => token(/[^\r\n]+/),
  },
});
