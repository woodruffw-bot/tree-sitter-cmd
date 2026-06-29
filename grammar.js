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
 *   - Keywords are case-insensitive and given elevated token precedence so they
 *     win ties against barewords, while longer barewords still win by maximal
 *     munch (so `rem` is a comment but `remote` is a command).
 *   - Command operators bind, lowest to highest: `&` < `||` < `&&` < `|`
 *     (matching ReactOS's `OpString` ordering).
 *
 * @author tree-sitter-cmd contributors
 * @license MIT
 */

/* eslint-disable no-undef */

// Keyword/bareword disambiguation is handled by the `word` directive
// (keyword extraction): a keyword only matches when it is the whole word, so
// `set` is a keyword but `setlocal` is a command. No token precedence needed.

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

module.exports = grammar({
  name: 'cmd',

  externals: ($) => [
    $._concat,
    $._rem,
    $._block_open,
    $._block_close,
    $._lparen,
    $._rparen,
    $._caret_escape,
  ],

  // Keyword extraction: a keyword only matches when it spans an entire word.
  word: ($) => $._cmd_text,

  // `_expansion` is exposed as a supertype so queries can target any %…%/!…!
  // form with `(_expansion)` instead of enumerating every concrete node. (It is
  // transparent — it adds no nodes.) `_statement` can't be a supertype: its
  // hidden `_unit` member resolves to multiple visible nodes.
  supertypes: ($) => [$._expansion],

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

    _line_content: ($) => choice($._statement, $.label, $.colon_comment),

    _newline: ($) => token(/\r?\n/),

    // ---------------------------------------------------------------------
    // Statements and the command-operator ladder.
    // ---------------------------------------------------------------------
    _statement: ($) =>
      choice($._unit, $.seq_list, $.or_list, $.and_list, $.pipeline),

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
      seq(
        repeat(field('quiet', $.quiet)),
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

    // A parenthesised compound. Newlines inside act like `&`. The parentheses
    // are supplied by the external scanner (which tracks block vs literal-paren
    // nesting), so `echo (text)` is literal but `( echo a )` is a block.
    block: ($) =>
      prec.right(
        seq(
          alias($._block_open, '('),
          optional($._block_body),
          alias($._block_close, ')'),
          repeat(field('redirect', $.redirection)),
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
          repeat(field('redirect', $.redirection)),
          field('name', $.command_name),
          repeat(
            choice(field('argument', $.argument), field('redirect', $.redirection)),
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
          kw($, 'if'),
          optional(alias(opt('/i'), $.if_flag)),
          optional(alias(ci('not'), $.not)),
          field('condition', $._if_condition),
          field('consequence', $._if_body),
          optional(seq(kw($, 'else'), field('alternative', $._if_body))),
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
    // An IF operand is a word whose bare text stops at `=` so that `a==b`
    // tokenises as `a`, `==`, `b`. It may also be fully wrapped in parentheses,
    // the classic `if (%1)==()` idiom for tolerating empty/odd arguments.
    _if_operand: ($) =>
      choice(
        alias($._if_word, $.argument),
        alias(seq($._lparen, optional($._if_word), $._rparen), $.argument),
      ),
    _if_word: ($) => seq($._if_fragment, repeat(seq($._concat, $._if_fragment))),
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

    _if_body: ($) => $._unit,

    // ---------------------------------------------------------------------
    // Control flow — FOR
    // ---------------------------------------------------------------------
    // FOR [/D | /R [path] | /L | /F ["opts"]] %%v IN (set) DO <command>
    for_statement: ($) =>
      prec.right(
        seq(
          kw($, 'for'),
          optional(field('option', $.for_option)),
          field('variable', $.loop_variable),
          kw($, 'in'),
          alias($._block_open, '('),
          field('set', optional($.for_set)),
          alias($._block_close, ')'),
          kw($, 'do'),
          field('body', $._if_body),
        ),
      ),

    // FOR options. `/R [path]` and `/F [options]` each take one optional word.
    // The option may be quoted (`/f "tokens=2 delims=,"`) or caret-escaped and
    // unquoted (`/f tokens^=2-5^ delims^=.-_`). A tree-sitter lexer-state
    // asymmetry lets only one for-flag accept a bareword argument, so `/R`/`/F`
    // share a single rule with one optional `_for_arg` (which also admits the
    // command-name word token the lexer offers in that state).
    for_option: ($) =>
      choice(
        alias(opt('/d'), $.for_flag),
        alias(opt('/l'), $.for_flag),
        seq(
          alias(choice(opt('/r'), opt('/f')), $.for_flag),
          optional(field('argument', alias($._for_arg, $.argument))),
        ),
      ),

    _for_arg: ($) =>
      seq(
        choice(alias($._cmd_text, $.text), $._fragment),
        repeat(seq($._concat, $._fragment)),
      ),

    for_set: ($) =>
      repeat1(
        choice(
          $.argument,
          $.backquote_string,
          $.single_quote_string,
          $._newline,
        ),
      ),

    // `command` source for FOR /F (backquoted). May be unterminated.
    backquote_string: ($) => token(/`[^`\r\n]*`?/),
    // 'command' source for FOR /F (single-quoted). The closing quote is required
    // so a stray apostrophe in a plain FOR set (`for %%a in (it's)`) stays text;
    // a properly quoted command keeps its inner parens/operators literal, e.g.
    // `for /f %%a in ('wmic … where (x=1) …') do …`.
    single_quote_string: ($) => token(/'[^'\r\n]*'/),

    // ---------------------------------------------------------------------
    // GOTO / CALL
    // ---------------------------------------------------------------------
    // GOTO label  /  GOTO :EOF. cmd ignores trailing tokens/separators after
    // the label (e.g. `goto :loop ;`), so tolerate trailing arguments.
    goto_statement: ($) =>
      prec.right(
        seq(
          kw($, 'goto'),
          optional(
            seq(
              field('target', $.argument),
              repeat(field('argument', $.argument)),
            ),
          ),
          repeat(field('redirect', $.redirection)),
        ),
      ),

    // CALL :label args  /  CALL file args  /  CALL command. Redirections may
    // follow (e.g. `call "%~f0" %* <input`).
    call_statement: ($) =>
      prec.right(
        seq(
          kw($, 'call'),
          repeat(
            choice(field('argument', $.argument), field('redirect', $.redirection)),
          ),
        ),
      ),

    // ---------------------------------------------------------------------
    // SET family
    // ---------------------------------------------------------------------
    set_statement: ($) =>
      prec.right(
        seq(
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
          repeat(field('redirect', $.redirection)),
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
          opt('/p'),
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
    set_arith: ($) => seq(opt('/a'), repeat(field('expression', $.argument))),
    // SET "name=value" — quoted assignment. cmd treats the span up to the last
    // quote as name=value, so the value may itself contain quotes. We model it
    // as a quote-led word run, which also stops cleanly at an operator/newline.
    set_quoted: ($) => prec.right(seq($.string, repeat($._fragment))),
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
    redirection: ($) => choice($.redirect_file, $.redirect_dup),

    // `>`, `>>`, `<`, with an optional immediately-preceding fd digit.
    redirect_file: ($) =>
      seq(field('operator', $.redirect_operator), field('target', $.argument)),
    redirect_operator: ($) => token(/[0-9]?(?:>>|>|<)/),

    // Handle duplication: `2>&1`, `>&2`, `<&3`.
    redirect_dup: ($) =>
      seq(
        field('operator', $.redirect_dup_operator),
        field('target', alias(token.immediate(/[0-9]/), $.file_descriptor)),
      ),
    redirect_dup_operator: ($) => token(/[0-9]?[<>]&/),

    // ---------------------------------------------------------------------
    // Words: command names and arguments are runs of adjacent fragments.
    // ---------------------------------------------------------------------
    command_name: ($) =>
      seq($._cmd_lead, repeat(seq($._concat, $._fragment))),

    // The first fragment of a command name may not begin with `@` (quiet) or
    // `:` (label). A lone `%`/`!` sigil can also lead a name: cmd happily parses
    // `! echo ...` as a command named `!` (it fails at runtime — a common
    // debug-disable trick), and `%`/`!` only form an expansion when they pair up.
    _cmd_lead: ($) =>
      choice($._cmd_text, $.string, $._expansion, alias($._stray_sigil, $.text)),
    // cmd ends an internal-command name at `:` (and `.\,/;=[]`), so `goto:eof`
    // is `goto` + `:eof` and `call:sub` is `call` + `:sub`. We honour the `:`
    // break: a bareword stops at `:`, EXCEPT a leading drive letter keeps it
    // (`C:`, `C:\tools\foo.exe`), so drive-relative command paths still parse.
    _cmd_text: ($) =>
      token(
        choice(
          /[A-Za-z]:[^ \t\r\n&|<>()^"%!]*/,
          /[^ \t\r\n&|<>()^"%!@:][^ \t\r\n&|<>()^"%!:]*/,
        ),
      ),

    argument: ($) => seq($._fragment, repeat(seq($._concat, $._fragment))),

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

    // cmd does not strip quotes; they only group. An unterminated quote runs to
    // end of line. Modelled as a single token (the interior is opaque for now —
    // cmd does still expand %VAR% inside quotes, but we do not sub-node it yet).
    string: ($) => token(/"[^"\r\n]*"?/),

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

    rem_comment: ($) =>
      seq(
        alias($._rem, $.keyword),
        optional(alias($._line_text, $.comment_text)),
      ),
    colon_comment: ($) =>
      seq(token(/::/), optional(alias($._line_text, $.comment_text))),
    _line_text: ($) => token(/[^\r\n]+/),
  },
});
