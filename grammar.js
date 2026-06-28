/**
 * tree-sitter-cmd — a grammar for Windows cmd.exe / batch (.bat, .cmd) scripts.
 *
 * Design notes live in GRAMMAR_DESIGN.md. The short version:
 *
 *   - cmd.exe has no whole-file grammar; each physical line is expanded, parsed
 *     and executed in turn. We model the whole file as one CST for tooling.
 *   - `extras` is `[ \t]` plus caret line-continuations only. Newlines are
 *     significant statement terminators.
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
const KW_PREC = 0;

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

/** Case-insensitive keyword as a single elevated-precedence token. */
function ci(word) {
  return token(prec(KW_PREC, new RegExp(ciSource(word))));
}

/**
 * Case-insensitive keyword aliased to a named `keyword` node so it can be
 * targeted by highlight queries (regex tokens are otherwise anonymous and have
 * no fixed text to match on).
 */
function kw(word) {
  return alias(ci(word), 'keyword');
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
  ],

  // Keyword extraction: a keyword only matches when it spans an entire word.
  word: ($) => $._cmd_text,

  extras: ($) => [/[ \t]/, $.line_continuation],

  conflicts: ($) => [[$.for_option]],

  rules: {
    // A batch file is a sequence of physical lines. Every line is terminated by
    // a newline except, optionally, the last one.
    program: ($) => seq(repeat($._line), optional($._line_content)),

    _line: ($) => seq(optional($._line_content), $._newline),

    _line_content: ($) => choice($._statement, $.label, $.colon_comment),

    _newline: ($) => token(/\r?\n/),

    // A caret immediately before a newline splices the next physical line.
    line_continuation: ($) => token(/\^\r?\n[ \t]*/),

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
    // plain command (e.g. `@rem`, `@if`, `@echo off`).
    _unit: ($) =>
      seq(
        optional(field('quiet', $.quiet)),
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
          kw('if'),
          optional(alias(opt('/i'), $.if_flag)),
          optional(alias(ci('not'), $.not)),
          field('condition', $._if_condition),
          field('consequence', $._if_body),
          optional(seq(kw('else'), field('alternative', $._if_body))),
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
        field('arg', $._if_operand),
      ),

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
          kw('for'),
          optional(field('option', $.for_option)),
          field('variable', $.loop_variable),
          kw('in'),
          alias($._block_open, '('),
          field('set', optional($.for_set)),
          alias($._block_close, ')'),
          kw('do'),
          field('body', $._if_body),
        ),
      ),

    for_option: ($) =>
      choice(
        opt('/d'),
        seq(opt('/r'), optional(field('path', $.argument))),
        opt('/l'),
        seq(opt('/f'), optional(field('options', $.string))),
      ),

    for_set: ($) =>
      repeat1(choice($.argument, $.backquote_string, $._newline)),

    // `command` source for FOR /F.
    backquote_string: ($) => token(/`[^`\r\n]*`?/),

    // ---------------------------------------------------------------------
    // GOTO / CALL
    // ---------------------------------------------------------------------
    // GOTO label  /  GOTO :EOF
    goto_statement: ($) =>
      seq(kw('goto'), optional(field('target', $.argument))),

    // CALL :label args  /  CALL file args  /  CALL command
    call_statement: ($) =>
      prec.right(seq(kw('call'), repeat(field('argument', $.argument)))),

    // ---------------------------------------------------------------------
    // SET family
    // ---------------------------------------------------------------------
    set_statement: ($) =>
      prec.right(
        seq(
          kw('set'),
          optional(
            choice(
              $.set_prompt,
              $.set_arith,
              $.set_assignment,
              $.string,
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
    // SET /P name=prompt
    set_prompt: ($) =>
      seq(
        opt('/p'),
        field('name', alias($._set_name, $.variable_name)),
        '=',
        repeat(field('prompt', $.argument)),
      ),
    // SET /A expression  (refined to an arithmetic sub-grammar in M7)
    set_arith: ($) => seq(opt('/a'), repeat(field('expression', $.argument))),
    // SET  /  SET prefix  (display)
    set_display: ($) => alias($._set_name, $.variable_name),

    // A SET variable name stops at `=`.
    _set_name: ($) => token(/[^ \t\r\n&|<>()^"%!=]+/),

    // ---------------------------------------------------------------------
    // Redirections
    // ---------------------------------------------------------------------
    redirection: ($) => choice($.redirect_file, $.redirect_dup),

    // `>`, `>>`, `<`, with an optional immediately-preceding fd digit.
    redirect_file: ($) =>
      seq(field('op', $.redirect_operator), field('target', $.argument)),
    redirect_operator: ($) => token(/[0-9]?(?:>>|>|<)/),

    // Handle duplication: `2>&1`, `>&2`, `<&3`.
    redirect_dup: ($) =>
      seq(
        field('op', $.redirect_dup_operator),
        field('target', alias(token.immediate(/[0-9]/), $.file_descriptor)),
      ),
    redirect_dup_operator: ($) => token(/[0-9]?[<>]&/),

    // ---------------------------------------------------------------------
    // Words: command names and arguments are runs of adjacent fragments.
    // ---------------------------------------------------------------------
    command_name: ($) =>
      seq($._cmd_lead, repeat(seq($._concat, $._fragment))),

    // The first fragment of a command name may not begin with `@` (quiet) or
    // `:` (label), but may otherwise contain `:` (drive letters, `c:\...`).
    _cmd_lead: ($) => choice($._cmd_text, $.string, $._expansion),
    _cmd_text: ($) => token(/[^ \t\r\n&|<>()^"%!@:][^ \t\r\n&|<>()^"%!]*/),

    argument: ($) => seq($._fragment, repeat(seq($._concat, $._fragment))),

    _fragment: ($) =>
      choice(
        $.text,
        $.string,
        $.escape_sequence,
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

    // A caret escapes the single following (non-newline) character.
    escape_sequence: ($) => token(/\^[^\r\n]/),

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
      token(new RegExp('%%(?:~' + TILDE_MODS + PATH_SEARCH + ')?[A-Za-z]')),
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
