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

/** Attach one or more `@` prefixes to the concrete statement they suppress. */
function quietPrefix($) {
  return repeat(field('quiet', $.quiet));
}

module.exports = grammar({
  name: 'cmd',

  externals: ($) => [
    $._concat,
    $._rem,
    $._redirect_source,
    $._block_open,
    $._block_close,
    $._lparen,
    $._rparen,
    $._caret_escape,
    $._string_end,
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

  conflicts: ($) => [[$.for_option]],

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
        seq(field('left', $._statement), '||', field('right', $._statement)),
      ),

    and_list: ($) =>
      prec.left(
        PREC.AND,
        seq(field('left', $._statement), '&&', field('right', $._statement)),
      ),

    pipeline: ($) =>
      prec.left(
        PREC.PIPE,
        seq(field('left', $._statement), '|', field('right', $._statement)),
      ),

    // A `@` echo-suppress prefix may precede any command form, not just a
    // plain command (e.g. `@rem`, `@if`, `@echo off`). cmd accepts the prefix
    // stacked (`@@fc ...`), each `@` a redundant suppression, so allow a run.
    _unit: ($) =>
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

    // A parenthesised compound. Newlines inside act like `&`. Redirections may
    // appear before or after the block. The parentheses are supplied by the
    // external scanner (which tracks block vs literal-paren nesting), so
    // `echo (text)` is literal but `( echo a )` is a block.
    block: ($) =>
      prec.right(
        seq(
          quietPrefix($),
          repeat(field('redirect', $._redirection)),
          alias($._block_open, '('),
          optional($._block_body),
          alias($._block_close, ')'),
          repeat(field('redirect', $._redirection)),
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
          repeat(field('redirect', $._redirection)),
          field('name', $.command_name),
          repeat(
            choice(
              field('argument', $.argument),
              field('redirect', $._redirection),
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
          optional(alias(opt('/i'), $.if_flag)),
          optional(alias(ci('not'), $.not)),
          field('condition', $._if_condition),
          // cmd parses the rest of each branch through its operator ladder.
          // `prec.right` keeps a following ELSE attached to this IF.
          field('consequence', $._statement),
          optional(seq(kw($, 'else'), field('alternative', $._statement))),
        ),
      ),

    _if_condition: ($) => choice($.comparison, $.unary_condition),

    comparison: ($) =>
      seq(
        field('left', $._if_operand),
        field('operator', $.comparison_operator),
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
        field('argument', $._if_operand),
      ),

    // An IF operand is a word whose bare text stops at `=` so that `a==b`
    // tokenises as `a`, `==`, `b`. It may also be fully wrapped in parentheses,
    // the classic `if (%1)==()` idiom for tolerating empty/odd arguments.
    _if_operand: ($) =>
      choice(
        alias($._if_word, $.argument),
        alias(seq($._lparen, optional($._if_word), $._rparen), $.argument),
      ),
    _if_word: ($) => wordOf($, $._if_fragment),
    _if_fragment: ($) =>
      choice(
        $._if_text,
        $.string,
        $.escape_sequence,
        // A lone caret escaping a following `%`/`!` (e.g. `if ^%V:~0,1% …`).
        alias($._caret_escape, $.escape_sequence),
        $._expansion,
        alias($._stray_sigil, $.text),
      ),
    _if_text: ($) => token(/[^ \t\r\n&|<>()^"%!=]+/),

    // ---------------------------------------------------------------------
    // Control flow — FOR
    // ---------------------------------------------------------------------
    // FOR [/D | /R [path] | /L | /F ["opts"]] %%v IN (set) DO <command>
    for_statement: ($) =>
      prec.right(
        seq(
          quietPrefix($),
          kw($, 'for'),
          optional(field('option', $.for_option)),
          field('variable', $.loop_variable),
          kw($, 'in'),
          alias($._block_open, '('),
          field('set', optional($.for_set)),
          alias($._block_close, ')'),
          kw($, 'do'),
          // Operators after DO remain inside the loop body.
          field('body', $._statement),
        ),
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
          optional(field('argument', alias($._for_arg, $.argument))),
        ),
        seq(
          alias(opt('/d'), $.for_flag),
          optional(
            seq(
              alias(opt('/r'), $.for_flag),
              optional(field('argument', alias($._for_arg, $.argument))),
            ),
          ),
        ),
        seq(
          alias(opt('/r'), $.for_flag),
          optional(
            choice(
              seq(
                alias(opt('/d'), $.for_flag),
                optional(field('argument', alias($._for_arg, $.argument))),
              ),
              seq(
                field('argument', alias($._for_arg, $.argument)),
                optional(alias(opt('/d'), $.for_flag)),
              ),
            ),
          ),
        ),
      ),

    _for_arg: ($) =>
      wordOf($, $._for_arg_lead, $._fragment),
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
      token(/[^/ \t\r\n&|<>()^"%!][^ \t\r\n&|<>()^"%!]*/),

    for_set: ($) =>
      repeat1(
        choice(
          $.argument,
          $.backquote_string,
          $.single_quote_string,
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
    // GOTO label  /  GOTO :EOF. cmd ignores trailing tokens/separators after
    // the label (e.g. `goto :loop ;`), so tolerate trailing arguments.
    goto_statement: ($) =>
      prec.right(
        seq(
          quietPrefix($),
          kw($, 'goto'),
          optional(
            seq(
              field('target', $.argument),
              repeat(field('argument', $.argument)),
            ),
          ),
          repeat(field('redirect', $._redirection)),
        ),
      ),

    // CALL :label args  /  CALL file args  /  CALL command. Redirections may
    // follow (e.g. `call "%~f0" %* <input`).
    call_statement: ($) =>
      prec.right(
        seq(
          quietPrefix($),
          kw($, 'call'),
          repeat(
            choice(
              field('argument', $.argument),
              field('redirect', $._redirection),
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
          repeat(field('redirect', $._redirection)),
          kw($, 'set'),
          optional(
            choice(
              $.set_prompt,
              $.set_arith,
              $.set_assignment,
              $.set_quoted,
              $.set_display,
            ),
          ),
          repeat(field('redirect', $._redirection)),
        ),
      ),

    // SET name=value  (the value is the rest of the logical line)
    set_assignment: ($) =>
      seq(
        field('name', alias($._set_name, $.variable_name)),
        '=',
        repeat(field('value', $.argument)),
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
          choice(
            seq(
              optional(field('name', alias($._set_name, $.variable_name))),
              '=',
              repeat(field('prompt', $.argument)),
            ),
            seq(field('prompt', $.string), repeat($._fragment)),
          ),
        ),
      ),
    // SET /A expression  (refined to an arithmetic sub-grammar in M7)
    set_arith: ($) =>
      seq(
        alias(opt('/a'), $.set_flag),
        repeat(field('expression', $.argument)),
      ),
    // SET "name=value". cmd treats the span up to the last quote as name=value,
    // so the value may itself contain quotes. Caret-escaped wrapper quotes are
    // also common in macro definitions and remain one quoted assignment.
    set_quoted: ($) =>
      prec.right(
        seq(choice($.string, $.caret_quoted_string), repeat($._fragment)),
      ),
    caret_quoted_string: ($) =>
      token(/\^"(?:[^\r\n^]|\^[^"\r\n])*(?:\^"|")/),
    // SET  /  SET prefix  (display)
    set_display: ($) => alias($._set_name, $.variable_name),

    // A SET variable name stops at `=` but may contain expansion sigils
    // (e.g. `err%%i` when building indexed variables inside a FOR loop) and even
    // internal spaces — cmd takes everything from after the switch up to the
    // first `=` as the name (`set sim salabim=magic` names "sim salabim"). The
    // first char is non-space so the name never starts on the separator after
    // `SET`, and the `/a`/`/p` switch tokens still win their slot by precedence.
    _set_name: ($) => token(/[^ \t\r\n&|<>()^"=][^\r\n&|<>()^"=]*/),

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
          field('target', $.argument),
        ),
        seq(
          field('operator', $.redirect_operator),
          field('target', $.argument),
        ),
      ),
    redirect_operator: ($) => token(/>>|>|</),

    // Handle duplication: `2>&1`, `>&2`, `<&3`.
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
          field('target', choice($.file_descriptor, $._expansion)),
        ),
        seq(
          field('operator', $.redirect_dup_operator),
          field('target', choice($.file_descriptor, $._expansion)),
        ),
      ),
    redirect_dup_operator: ($) => token(/[<>]&/),
    file_descriptor: ($) => token.immediate(/[0-9]/),

    // ---------------------------------------------------------------------
    // Words: command names and arguments are runs of adjacent fragments.
    // ---------------------------------------------------------------------
    command_name: ($) => wordOf($, $._cmd_lead, $._fragment),

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
            /[^ \t\r\n&|<>()^"%!@:,/;=\[\]\\]+[.\\][^ \t\r\n&|<>()^"%!]*/,
            /\.{1,2}[\\/][^ \t\r\n&|<>()^"%!]*/,
            /\\\\[^ \t\r\n&|<>()^"%!]*/,
          ),
        ),
      ),
    // A generic command may itself begin with one of cmd's internal-command
    // delimiters. Keep that fallback without letting the same token swallow a
    // delimiter after a recognized keyword such as `set/a`.
    _cmd_punct_lead: ($) =>
      token(/[.,/;=\[\]\\][^ \t\r\n&|<>()^"%!]*/),
    // cmd ends an internal-command name at `:.\,/;=[]`, so `goto:eof` is
    // `goto` + `:eof` and `set/a` is `set` + `/a`. Explicit paths and dotted
    // executable names are handled by `_cmd_path`, while a leading drive letter
    // remains one `_cmd_text` token.
    _cmd_text: ($) =>
      token(
        choice(
          /[A-Za-z]:[^ \t\r\n&|<>()^"%!]*/,
          /[^ \t\r\n&|<>()^"%!@:,./;=\[\]\\][^ \t\r\n&|<>()^"%!:,./;=\[\]\\]*/,
        ),
      ),

    argument: ($) => wordOf($, $._fragment),

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
    // %%x FOR loop variable (batch context), optionally with ~modifiers.
    loop_variable: ($) =>
      token(new RegExp('%%(?:~' + TILDE_MODS + PATH_SEARCH + ')?' + FOR_VAR)),
    // %% literal percent sign.
    percent_literal: ($) => token(/%%/),

    // ---------------------------------------------------------------------
    // Labels and comments
    // ---------------------------------------------------------------------
    label: ($) =>
      seq(
        token(/:/),
        field('name', alias($._label_name, $.label_name)),
        optional($._label_tail),
      ),
    _label_name: ($) => token.immediate(/[^ \t\r\n:+;=,&|<>()]+/),
    _label_tail: ($) => alias(token(/[^\r\n]+/), $.label_text),

    // Keep the body opaque. Expansion-shaped text inside REM is comment text,
    // not an expansion that tooling should treat as live code.
    rem_comment: ($) =>
      seq(
        quietPrefix($),
        repeat(field('redirect', $._redirection)),
        alias($._rem, $.keyword),
        optional(alias($._line_text, $.comment_text)),
      ),
    colon_comment: ($) =>
      choice(
        seq(token(/::/), optional(alias($._line_text, $.comment_text))),
        seq(
          token(prec(1, /:[^$0-9A-Za-z_\r\n:]/)),
          optional(alias($._line_text, $.comment_text)),
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
